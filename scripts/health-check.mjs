import { readFileSync } from "node:fs";

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

const pkg = readJson("package.json");
const tauri = readJson("src-tauri/tauri.conf.json");

const checks = [
  ["package name", pkg.name === "wt-media-desktop"],
  ["tauri product", tauri.productName === "WT Media"],
  ["tauri window", Array.isArray(tauri.app?.windows) && tauri.app.windows.length > 0],
];

const failed = checks.filter(([, ok]) => !ok).map(([name]) => name);

if (failed.length > 0) {
  console.error(`wt-media-desktop health failed: ${failed.join(", ")}`);
  process.exit(1);
}

console.log("wt-media-desktop health ok");
