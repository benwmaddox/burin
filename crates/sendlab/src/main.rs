//! sendlab: how cheaply can a page made of many immutable ranges go out over TCP (loopback)?
//! The sender's CPU per page (its thread's user + kernel time) for each way of sending:
//!
//!   writev     what burin does: one call, the kernel copies every byte
//!   zerocopy   sendmsg(MSG_ZEROCOPY): pages pinned, completions read from the error queue
//!   sendfile   the ranges in a memfd (page cache), one sendfile per range
//!   sendfile1  the whole page as one contiguous range in a memfd (a best case)
//!   sendfilec  as sendfile, corked (TCP_CORK) so the ranges leave as full packets
//!   sendfile5  corked, the page as 5 ranges (the messages adjacent in the file, as an
//!              append-only store would hold an oldest-first window)
//!   splice     vmsplice the ranges into a pipe, splice the pipe to the socket
//!
//!   sendlab <method> <raw|gz> [pages]
//! Sender on core 3, reader on core 5.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;

fn pin(core: usize) {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(core, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

fn thread_cpu() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9
}

/// The page's ranges, sized like burin's room page: a head, 40 messages, a tail, a sidebar in
/// pieces with its splices, an end (raw: 345 KB in 53 ranges; gz: 74 KB in 57).
fn ranges(gz: bool) -> Vec<Vec<u8>> {
    let mut sizes = vec![if gz { 1200 } else { 3400 }];
    for i in 0..40 {
        sizes.push(if gz { 1650 + (i % 7) * 10 } else { 8300 + (i % 7) * 50 });
    }
    sizes.push(if gz { 1300 } else { 4100 });
    for _ in 0..4 {
        sizes.push(if gz { 900 } else { 2300 });
        sizes.push(if gz { 70 } else { 90 });
    }
    sizes.push(if gz { 200 } else { 900 });
    if gz {
        // the gzip header, final block and trailer
        sizes.insert(0, 10);
        sizes.push(2);
        sizes.push(8);
    }
    sizes.iter().enumerate().map(|(i, n)| (0..*n).map(|k| ((k * 31 + i * 7) % 251) as u8).collect()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let method = args.get(1).map(String::as_str).unwrap_or("writev").to_string();
    let gz = args.get(2).map(|s| s == "gz").unwrap_or(false);
    let pages: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let segs = ranges(gz);
    let page: usize = segs.iter().map(|s| s.len()).sum();

    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let total = ((page + if method == "hybrid" { 200 } else { 0 }) * pages) as u64;
    let reader = std::thread::spawn(move || {
        pin(5);
        let (mut s, _) = l.accept().unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let mut got = 0u64;
        let c0 = thread_cpu();
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got += n as u64,
                Err(e) => panic!("{e}"),
            }
        }
        (got, thread_cpu() - c0)
    });

    pin(3);
    let s = TcpStream::connect(addr).unwrap();
    s.set_nodelay(true).unwrap();
    let fd = s.as_raw_fd();
    let mut copied = 0u64;
    let mut completions = 0u64;
    // a memfd holding the ranges (for sendfile), and their offsets
    let (mfd, offs) = if method.starts_with("sendfile") || method == "hybrid" {
        let name = std::ffi::CString::new("page").unwrap();
        let m = unsafe { libc::memfd_create(name.as_ptr(), 0) };
        let mut offs = vec![];
        let mut at = 0i64;
        for sg in &segs {
            unsafe { libc::pwrite(m, sg.as_ptr() as *const _, sg.len(), at) };
            offs.push(at);
            at += sg.len() as i64;
        }
        (m, offs)
    } else {
        (-1, vec![])
    };
    let mut pipe = [0i32; 2];
    if method == "splice" {
        unsafe {
            libc::pipe2(pipe.as_mut_ptr(), 0);
            libc::fcntl(pipe[1], libc::F_SETPIPE_SZ, 1 << 20);
        }
    }
    if method == "zerocopy" {
        let one: i32 = 1;
        let r = unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_ZEROCOPY, &one as *const i32 as *const _, 4) };
        assert_eq!(r, 0, "SO_ZEROCOPY");
    }
    let iov: Vec<libc::iovec> = segs.iter().map(|s| libc::iovec { iov_base: s.as_ptr() as *mut _, iov_len: s.len() }).collect();

    let w0 = std::time::Instant::now();
    let c0 = thread_cpu();
    for _ in 0..pages {
        match method.as_str() {
            "writev" | "zerocopy" => {
                // send the whole iovec list, resuming after partial sends
                let mut v = iov.clone();
                let mut at = 0;
                while at < v.len() {
                    let mut h: libc::msghdr = unsafe { std::mem::zeroed() };
                    h.msg_iov = v[at..].as_mut_ptr();
                    h.msg_iovlen = (v.len() - at).min(1024);
                    let flags = if method == "zerocopy" { libc::MSG_ZEROCOPY } else { 0 };
                    let mut n = unsafe { libc::sendmsg(fd, &h, flags) };
                    if n < 0 {
                        let e = std::io::Error::last_os_error();
                        if e.raw_os_error() == Some(libc::ENOBUFS) {
                            // too many pending zerocopy sends: reap and retry
                            reap(fd, &mut completions, &mut copied, true);
                            continue;
                        }
                        panic!("sendmsg: {e}");
                    }
                    while n > 0 && at < v.len() {
                        if (n as usize) >= v[at].iov_len {
                            n -= v[at].iov_len as isize;
                            at += 1;
                        } else {
                            v[at].iov_base = unsafe { (v[at].iov_base as *mut u8).add(n as usize) } as *mut _;
                            v[at].iov_len -= n as usize;
                            n = 0;
                        }
                    }
                }
                if method == "zerocopy" {
                    reap(fd, &mut completions, &mut copied, false);
                }
            }
            "hybrid" => {
                // the best a real page could do: headers (MSG_MORE), the shared prefix (gzip header,
                // head, 40 messages, tail) as one sendfile, the per-viewer rest as one writev
                let hdr = [0u8; 200];
                let n = unsafe { libc::send(fd, hdr.as_ptr() as *const _, hdr.len(), libc::MSG_MORE) };
                assert_eq!(n, 200);
                let cut = 42 + gz as usize;
                let mut off = 0i64;
                let mut left: usize = segs[..cut].iter().map(|s| s.len()).sum();
                while left > 0 {
                    let n = unsafe { libc::sendfile(fd, mfd, &mut off, left) };
                    assert!(n > 0, "sendfile");
                    left -= n as usize;
                }
                let mut v = iov[cut..].to_vec();
                let mut at = 0;
                while at < v.len() {
                    let mut n = unsafe { libc::writev(fd, v[at..].as_ptr(), (v.len() - at) as i32) };
                    assert!(n > 0, "writev");
                    while n > 0 && at < v.len() {
                        if (n as usize) >= v[at].iov_len {
                            n -= v[at].iov_len as isize;
                            at += 1;
                        } else {
                            v[at].iov_base = unsafe { (v[at].iov_base as *mut u8).add(n as usize) } as *mut _;
                            v[at].iov_len -= n as usize;
                            n = 0;
                        }
                    }
                }
            }
            "sendfile5" => {
                cork(fd, 1);
                // head | 40 messages | tail | sidebar | end, cut where the real page would be
                let g = gz as usize;
                let cuts = [0usize, 1 + g, 41 + g, 42 + g, segs.len() - 1 - 2 * g, segs.len()];
                for w in cuts.windows(2) {
                    let mut off = offs[w[0]];
                    let mut left: usize = segs[w[0]..w[1]].iter().map(|s| s.len()).sum();
                    while left > 0 {
                        let n = unsafe { libc::sendfile(fd, mfd, &mut off, left) };
                        assert!(n > 0, "sendfile");
                        left -= n as usize;
                    }
                }
                cork(fd, 0);
            }
            "sendfile" | "sendfilec" => {
                if method == "sendfilec" {
                    cork(fd, 1);
                }
                for (k, sg) in segs.iter().enumerate() {
                    let mut off = offs[k];
                    let mut left = sg.len();
                    while left > 0 {
                        let n = unsafe { libc::sendfile(fd, mfd, &mut off, left) };
                        assert!(n > 0, "sendfile");
                        left -= n as usize;
                    }
                }
                if method == "sendfilec" {
                    cork(fd, 0);
                }
            }
            "sendfile1" => {
                let mut off = 0i64;
                let mut left = page;
                while left > 0 {
                    let n = unsafe { libc::sendfile(fd, mfd, &mut off, left) };
                    assert!(n > 0, "sendfile");
                    left -= n as usize;
                }
            }
            "splice" => {
                // into the pipe without copying (the ranges never change), then out to the socket
                let mut at = 0;
                while at < iov.len() {
                    let mut batch = vec![];
                    let mut bytes = 0;
                    while at < iov.len() && bytes + iov[at].iov_len <= (1 << 20) {
                        batch.push(iov[at]);
                        bytes += iov[at].iov_len;
                        at += 1;
                    }
                    let mut v = batch.clone();
                    let mut i = 0;
                    while i < v.len() {
                        let mut n = unsafe { libc::vmsplice(pipe[1], v[i..].as_ptr(), v.len() - i, 0) };
                        assert!(n > 0, "vmsplice");
                        while n > 0 && i < v.len() {
                            if (n as usize) >= v[i].iov_len {
                                n -= v[i].iov_len as isize;
                                i += 1;
                            } else {
                                v[i].iov_base = unsafe { (v[i].iov_base as *mut u8).add(n as usize) } as *mut _;
                                v[i].iov_len -= n as usize;
                                n = 0;
                            }
                        }
                    }
                    let mut left = bytes;
                    while left > 0 {
                        let n = unsafe { libc::splice(pipe[0], std::ptr::null_mut(), fd, std::ptr::null_mut(), left, libc::SPLICE_F_MOVE) };
                        assert!(n > 0, "splice");
                        left -= n as usize;
                    }
                }
            }
            m => panic!("unknown method {m}"),
        }
    }
    if method == "zerocopy" {
        // the last completions
        for _ in 0..1000 {
            reap(fd, &mut completions, &mut copied, false);
            if completions >= pages as u64 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let cpu = thread_cpu() - c0;
    let wall = w0.elapsed().as_secs_f64();
    drop(s);
    let (got, rcpu) = reader.join().unwrap();
    assert_eq!(got, total, "bytes received");
    println!(
        "{method:9} {:3}: {:6.1} us sender CPU/page, {:6.1} us reader CPU/page, {:7.0} pages/s, {:5.2} GB/s{}",
        if gz { "gz" } else { "raw" },
        cpu * 1e6 / pages as f64,
        rcpu * 1e6 / pages as f64,
        pages as f64 / wall,
        total as f64 / wall / 1e9,
        if method == "zerocopy" { format!(", sends the kernel copied anyway: {copied} of {completions} completions") } else { String::new() }
    );
}

/// TCP_CORK on or off: while on, the kernel holds partial packets; off sends what's left.
fn cork(fd: i32, on: i32) {
    unsafe { libc::setsockopt(fd, libc::IPPROTO_TCP, libc::TCP_CORK, &on as *const i32 as *const _, 4) };
}

/// Reads zerocopy completions from the socket's error queue (blocking only when asked).
fn reap(fd: i32, completions: &mut u64, copied: &mut u64, block: bool) {
    loop {
        let mut ctrl = [0u8; 128];
        let mut h: libc::msghdr = unsafe { std::mem::zeroed() };
        h.msg_control = ctrl.as_mut_ptr() as *mut _;
        h.msg_controllen = ctrl.len();
        let r = unsafe { libc::recvmsg(fd, &mut h, libc::MSG_ERRQUEUE | if block { 0 } else { libc::MSG_DONTWAIT }) };
        if r < 0 {
            if block {
                std::thread::sleep(std::time::Duration::from_micros(50));
                continue;
            }
            return;
        }
        let cm = unsafe { libc::CMSG_FIRSTHDR(&h) };
        if !cm.is_null() {
            let ee = unsafe { &*(libc::CMSG_DATA(cm) as *const libc::sock_extended_err) };
            // a range of completed sends: [ee_info, ee_data]
            *completions += (ee.ee_data - ee.ee_info + 1) as u64;
            // SO_EE_CODE_ZEROCOPY_COPIED (linux/errqueue.h; not in the libc crate)
            if ee.ee_code & 1 != 0 {
                *copied += (ee.ee_data - ee.ee_info + 1) as u64;
            }
        }
        if block {
            return;
        }
    }
}
