// amigo-blinzel website: the download button offers the file for the
// visitor's system. Everything runs in the browser; nothing is sent or
// stored.
"use strict";

// The release asset for this browser's system, or null (phones, tablets,
// unknown systems keep the link to the download section).
async function platform() {
  const ua = navigator.userAgent;
  const uad = navigator.userAgentData;
  if (uad?.mobile || /Android|iPhone|iPad|iPod/i.test(ua)) return null;
  // iPadOS presents itself as a Mac, but with a touch screen.
  if (/Mac/i.test(ua) && navigator.maxTouchPoints > 1) return null;
  let arch = "";
  if (uad?.getHighEntropyValues) {
    try {
      arch = (await uad.getHighEntropyValues(["architecture"])).architecture || "";
    } catch {
      // Not offered: fall back to the user agent string.
    }
  }
  const os = uad?.platform || ua;
  const arm = arch === "arm" || /aarch64|arm64/i.test(ua);
  if (/Win/i.test(os)) return arm ? "arm64" : "x64";
  if (/Mac/i.test(os)) return "macos";
  if (/Linux|X11/i.test(os) && !/CrOS/i.test(ua)) return arm ? "linux-arm64" : "linux-x64";
  return null;
}

async function download(link) {
  const key = await platform();
  const href = key && link.getAttribute(`data-${key}`);
  const label = key && link.getAttribute(`data-label-${key}`);
  if (href && label) {
    link.href = href;
    link.textContent = label;
  }
}

for (const link of document.querySelectorAll("a[data-download]")) download(link);
