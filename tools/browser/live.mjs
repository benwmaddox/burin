// Live updates in real browsers (Chromium, headless): two people on the same room. David posts,
// edits, boosts and deletes from his page; Jason's page shows each change without reloading
// (Turbo applies what the stream sends), and David's own page marks his new message as his,
// with its edit controls, as the page's script does for messages served with the page.
//   node live.mjs <base url> <David's token> <Jason's token> <room id>
import { chromium } from "playwright-core";
import { homedir } from "node:os";

const [base, davidToken, jasonToken, room] = process.argv.slice(2);
const chrome = process.env.CHROME || `${homedir()}/.cache/ms-playwright/chromium-1208/chrome-linux64/chrome`;
let failed = false;
const want = (what, ok, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ": " + detail : ""}`);
  if (!ok) failed = true;
};

const browser = await chromium.launch({ executablePath: chrome, headless: true });
const open = async (token) => {
  const ctx = await browser.newContext();
  await ctx.addCookies([{ name: "session_token", value: token, domain: new URL(base).hostname, path: "/" }]);
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`${base}/rooms/${room}`);
  await page.waitForSelector("#live_room_messages");
  // the stream connects
  await page.waitForFunction(() => document.querySelector("turbo-stream-source")?.isConnected, null, { timeout: 5000 });
  await page.waitForTimeout(500);
  await page.evaluate(() => (window.__burinSameDocument = true));
  return { page, errors };
};
const has = (page, text, extra = "") => page.waitForFunction(([t, x]) => [...document.querySelectorAll(".message" + x)].some((m) => m.textContent.includes(t)), [text, extra], { timeout: 8000 }).then(() => true, () => false);
const gone = (page, text) => page.waitForFunction((t) => ![...document.querySelectorAll(".message")].some((m) => m.textContent.includes(t)), text, { timeout: 8000 }).then(() => true, () => false);

try {
  const david = await open(davidToken);
  const jason = await open(jasonToken);
  const text = `live in a browser ${Date.now()}`;

  await david.page.fill("#composer textarea", text);
  await david.page.press("#composer textarea", "Enter");
  want("Jason sees David's post arrive", await has(jason.page, text));
  want("…not as his own", await jason.page.evaluate((t) => [...document.querySelectorAll(".message")].filter((m) => m.textContent.includes(t)).every((m) => !m.classList.contains("message--me")), text));
  want("David's page marks it as his", await has(david.page, text, ".message--me"));
  want("…with its edit controls", await david.page.evaluate((t) => [...document.querySelectorAll(".message--me")].some((m) => m.textContent.includes(t) && m.querySelector(".message__edit:not([hidden])")), text));

  // the edit and the boost, from David's message's own forms (as a person clicks them)
  const id = await david.page.evaluate((t) => [...document.querySelectorAll(".message--me")].find((m) => m.textContent.includes(t)).dataset.messageId, text);
  const post = (path, fields) => david.page.evaluate(async ([p, f]) => (await fetch(p, { method: "POST", body: new URLSearchParams(f), redirect: "manual" })).status, [path, fields]);
  await post("/_do/edit_message", { __arg_message: `#${id}`, body: `${text} (edited)` });
  want("Jason sees the edit", await has(jason.page, `${text} (edited)`));
  await post("/_do/boost", { __arg_message: `#${id}`, __arg_content: "🎉" });
  want("Jason sees the boost", await jason.page.waitForFunction((i) => document.querySelector(`.message[data-message-id="${i}"] .boost`)?.textContent.includes("🎉"), id, { timeout: 8000 }).then(() => true, () => false));
  await post("/_do/delete_message", { __arg_message: `#${id}` });
  want("Jason sees the delete", await gone(jason.page, text));
  want("neither page reloaded", (await jason.page.evaluate(() => window.__burinSameDocument === true)) && (await david.page.evaluate(() => window.__burinSameDocument === true)));
  want("no script errors", jason.errors.length + david.errors.length === 0, [...jason.errors, ...david.errors].slice(0, 3).join(" | "));
} catch (e) {
  want("the run", false, String(e).split("\n")[0]);
} finally {
  await browser.close();
}
console.log(failed ? "FAIL" : "PASS");
process.exitCode = failed ? 1 : 0;
