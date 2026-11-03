// Runs one of the website's scripts in a stand-in browser (Node's vm
// module), for tests/scripts.rs. Argument: the script; standard input: a
// JSON array of cases {ua, language, touch, uad: {platform, mobile, arch,
// throws}, attrs} (stdin, as Windows limits a command line to 32K).
// Prints one line per case: the download link's href and text after the
// script ran, and the address passed to location.replace, tab-separated.
"use strict";
const fs = require("fs");
const vm = require("vm");

const script = process.argv[2];
const json = fs.readFileSync(0, "utf8");
const source = fs.readFileSync(script, "utf8");

async function run(c) {
  const attrs = c.attrs || {};
  const link = {
    href: attrs.href ?? "",
    textContent: attrs.text ?? "",
    getAttribute: (name) => (name in attrs ? attrs[name] : null),
  };
  const navigator = { userAgent: c.ua ?? "", language: c.language, maxTouchPoints: c.touch ?? 0 };
  if (c.uad) {
    navigator.userAgentData = {
      platform: c.uad.platform,
      mobile: !!c.uad.mobile,
      getHighEntropyValues: async () => {
        if (c.uad.throws) throw new Error("not allowed");
        return { architecture: c.uad.arch ?? "" };
      },
    };
  }
  let replaced = "";
  const context = vm.createContext({
    navigator,
    document: { querySelectorAll: (sel) => (sel === "a[data-download]" ? [link] : []) },
    location: { replace: (to) => { replaced = to; } },
  });
  vm.runInContext(source, context, { filename: script });
  // The scripts' promises settle before the next macrotask.
  await new Promise((resolve) => setTimeout(resolve, 0));
  return [link.href, link.textContent, replaced].join("\t");
}

(async () => {
  const lines = [];
  for (const c of JSON.parse(json)) lines.push(await run(c));
  process.stdout.write(lines.join("\n") + "\n");
})();
