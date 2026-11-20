#!/usr/bin/env node
// Inspect the *isolated* browser launched by Compare-Browser-Gta.mjs.
// Optional --play follows the official "Play Now" link, never advertisements.
const port = Number(process.argv[2]);
const play = process.argv.includes("--play");
const dismissAds = process.argv.includes("--dismiss-overlays");
if (!Number.isInteger(port) || port <= 0) throw new Error("Specify local CDP port");
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const target = tabs.find((tab) => tab.type === "page" && /fiveplay/i.test(tab.url));
if (!target) throw new Error("No FivePlay tab in isolated browser");
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
const command = (method, params = {}) => new Promise((resolve, reject) => {
  const id = Math.round(Math.random() * 1e9);
  const listener = ({ data }) => {
    let msg;
    try { msg = JSON.parse(data); } catch { return; }
    if (msg.id !== id) return;
    socket.removeEventListener("message", listener);
    if (msg.error) reject(new Error(JSON.stringify(msg.error)));
    else resolve(msg.result);
  };
  socket.addEventListener("message", listener);
  socket.send(JSON.stringify({ id, method, params }));
});
const result = await command("Runtime.evaluate", {
  expression: `({
    title:document.title, url:location.href,
    links:[...document.querySelectorAll('a[href]')].map(a=>({
      text:a.innerText.trim().slice(0,70),
      href:a.href
    })).filter(x=>x.text).slice(0,40),
    buttons:[...document.querySelectorAll('button')].map(b=>b.innerText.trim()).slice(0,25),
    iframes:[...document.querySelectorAll('iframe')].map(f=>({
      url:f.src, id:f.id, className:f.className,
      parent:f.parentElement?.outerHTML.slice(0,270)
    })).slice(0,10),
  })`,
  returnByValue: true,
});
const details = result.result?.value;
console.log(JSON.stringify(details, null, 2));
if (play) {
  const link = details?.links?.find((a) =>
    /play now|jogar agora|version 2/i.test(a.text) &&
    new URL(a.href).hostname.endsWith("fiveplay.lol")
  );
  if (!link) throw new Error("No official FivePlay play link found");
  const selected = new URL(link.href);
  if (selected.protocol !== "https:") throw new Error("Unexpected game scheme");
  console.log("OPENING_OFFICIAL_GAME", selected.origin + selected.pathname);
  await command("Page.navigate", { url: selected.href });
}
if (dismissAds) {
  // Hide only 3rd-party ad frames in our disposable profiling profile;
  // no clicks, purchases or changes to the actual game canvas/shaders.
  const hidden = await command("Runtime.evaluate", {
    expression: `(() => {
      const frames = [...document.querySelectorAll('iframe')]
        .filter(f=>{try{return new URL(f.src).hostname==='alwingulla.com'}catch{return false}});
      frames.forEach(f=>{
        const element = f.closest('[class*="overlay"],[class*="modal"]') || f;
        element.style.setProperty('display','none','important');
      });
      return frames.length;
    })()`,
    returnByValue: true,
  });
  console.log("HIDDEN_AD_FRAMES", hidden.result?.value);
}
socket.close();
