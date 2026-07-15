import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawn } from "node:child_process";

const runtimeDir = ".runtime";
const pidPath = join(runtimeDir, "desktop-vite.pid");
const logPath = join(runtimeDir, "desktop-vite.log");
const url = process.env.WT_MEDIA_DESKTOP_DEV_URL ?? "http://127.0.0.1:5174/";

function readPid() {
  if (!existsSync(pidPath)) {
    return null;
  }
  const pid = Number(readFileSync(pidPath, "utf8").trim());
  return Number.isInteger(pid) && pid > 0 ? pid : null;
}

async function isHealthy() {
  try {
    const response = await fetch(url);
    const body = await response.text();
    return response.ok && body.includes("WT Media Desktop");
  } catch {
    return false;
  }
}

async function waitForHealth() {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (await isHealthy()) {
      return true;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  return false;
}

mkdirSync(runtimeDir, { recursive: true });

const existingPid = readPid();
if (existingPid && (await isHealthy())) {
  console.log(`wt-media-desktop dev server already running: pid=${existingPid} url=${url}`);
  process.exit(0);
}

const logFd = openSync(logPath, "a");
const child = spawn(
  process.execPath,
  ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "5174", "--strictPort"],
  {
    cwd: process.cwd(),
    detached: true,
    stdio: ["ignore", logFd, logFd],
  },
);

child.unref();
closeSync(logFd);
writeFileSync(pidPath, `${child.pid}\n`);

if (!(await waitForHealth())) {
  console.error(`wt-media-desktop dev server did not become healthy; see ${logPath}`);
  process.exit(1);
}

console.log(`wt-media-desktop dev server started: pid=${child.pid} url=${url}`);
