import { existsSync, readFileSync, unlinkSync } from "node:fs";

const pidPath = ".runtime/desktop-vite.pid";

if (!existsSync(pidPath)) {
  console.log("wt-media-desktop dev server is not running");
  process.exit(0);
}

const pid = Number(readFileSync(pidPath, "utf8").trim());
if (!Number.isInteger(pid) || pid <= 0) {
  unlinkSync(pidPath);
  console.log("wt-media-desktop removed invalid dev server pid file");
  process.exit(0);
}

try {
  process.kill(pid, "SIGTERM");
  console.log(`wt-media-desktop dev server stopped: pid=${pid}`);
} catch (error) {
  if (error.code !== "ESRCH") {
    console.error(`wt-media-desktop failed to stop dev server pid=${pid}: ${error.message}`);
    process.exit(1);
  }
  console.log(`wt-media-desktop dev server was already stopped: pid=${pid}`);
} finally {
  unlinkSync(pidPath);
}
