// A real browser on etch's Campfire (Chromium, headless): what Rails Campfire's pages do with
// Turbo, done here too. The page loads its sidebar once into a permanent frame; following a room
// link swaps the page without reloading it and keeps that sidebar (no second request); the
// page's script marks the viewer's own messages; posting from the composer shows the message.
//   node smoke.mjs <base url> <session token> <room id>
// Prints "ok"/"FAIL" lines, then PASS or FAIL (exit code 1).
import { chromium } from "playwright-core";
import { homedir } from "node:os";

const [base, token, room] = process.argv.slice(2);
const chrome = process.env.CHROME || `${homedir()}/.cache/ms-playwright/chromium-1208/chrome-linux64/chrome`;
let failed = false;
const want = (what, ok, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ": " + detail : ""}`);
  if (!ok) failed = true;
};

const browser = await chromium.launch({ executablePath: chrome, headless: true });
try {
  const ctx = await browser.newContext();
  const url = new URL(base);
  await ctx.addCookies([{ name: "session_token", value: token, domain: url.hostname, path: "/" }]);
  const page = await ctx.newPage();
  const requests = [];
  const bad = [];
  page.on("request", (r) => requests.push(r.url()));
  page.on("response", (r) => {
    if (r.status() >= 400 && !r.url().endsWith(".map")) bad.push(`${r.status()} ${r.url()}`);
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  const count = (re) => requests.filter((u) => re.test(u)).length;

  await page.goto(`${base}/rooms/${room}`);
  await page.waitForSelector("#user_sidebar .rooms a.room", { timeout: 10000 });
  want("the sidebar frame loads", true);
  want("Turbo runs", await page.evaluate(() => typeof window.Turbo === "object"));
  want("stylesheets apply", await page.evaluate(() => getComputedStyle(document.body).display !== "" && document.styleSheets.length > 20), `${await page.evaluate(() => document.styleSheets.length)} sheets`);
  want("one sidebar request", count(/\/users\/me\/sidebar/) === 1, String(count(/\/users\/me\/sidebar/)));
  want("the room being read isn't shown unread", await page.evaluate((r) => !document.querySelector(`#user_sidebar a[href="/rooms/${r}"].unread`), room));

  // mark the document and the sidebar's frame: a Turbo visit keeps both
  await page.evaluate(() => {
    window.__etchSameDocument = true;
    document.getElementById("user_sidebar").__etchSameFrame = true;
  });
  const next = await page.evaluate((r) => [...document.querySelectorAll("#user_sidebar .rooms a.room")].map((a) => a.getAttribute("href")).find((h) => h !== `/rooms/${r}`), room);
  want("another room to visit", !!next, next || "none");
  if (next) {
    const pages0 = count(/\/rooms\/\d+$/);
    await page.click(`#user_sidebar a[href="${next}"]`);
    await page.waitForFunction((h) => document.querySelector('meta[name="current-room-id"]')?.content === h.split("/").pop(), next, { timeout: 10000 });
    want("the visit fetched the page", count(/\/rooms\/\d+$/) === pages0 + 1);
    want("no full reload (Turbo Drive)", await page.evaluate(() => window.__etchSameDocument === true));
    want("the same sidebar, kept", await page.evaluate(() => document.getElementById("user_sidebar").__etchSameFrame === true));
    want("still one sidebar request", count(/\/users\/me\/sidebar/) === 1, String(count(/\/users\/me\/sidebar/)));
    want("the visited room isn't shown unread", await page.evaluate((h) => !document.querySelector(`#user_sidebar a[href="${h}"].unread`), next));
  }

  // post from the composer (Enter submits; Turbo follows the redirect and swaps the page)
  const text = `browser smoke ${Date.now()}`;
  await page.fill("#composer textarea", text);
  await page.press("#composer textarea", "Enter");
  await page.waitForFunction((t) => [...document.querySelectorAll(".message__presentation")].some((e) => e.textContent.includes(t)), text, { timeout: 10000 });
  want("a post shows in the room", true);
  want("it's marked as the viewer's own", await page.evaluate((t) => [...document.querySelectorAll(".message")].some((m) => m.classList.contains("message--me") && m.textContent.includes(t)), text));
  want("still no full reload", await page.evaluate(() => window.__etchSameDocument === true));
  want("still one sidebar request", count(/\/users\/me\/sidebar/) === 1, String(count(/\/users\/me\/sidebar/)));
  want("no failed requests", bad.length === 0, bad.slice(0, 5).join(", "));
  want("no script errors", errors.length === 0, errors.slice(0, 3).join(" | "));
} catch (e) {
  want("the run", false, String(e).split("\n")[0]);
} finally {
  await browser.close();
}
console.log(failed ? "FAIL" : "PASS");
process.exitCode = failed ? 1 : 0;
