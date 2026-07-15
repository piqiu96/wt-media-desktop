import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync("package.json", "utf8"));
const scripts = pkg.scripts ?? {};

const requiredScripts = [
  "bootstrap",
  "dev",
  "start",
  "stop",
  "test",
  "lint",
  "build",
  "package",
  "verify",
  "health",
];

const realCommandHints = {
  bootstrap: ["npm ci"],
  dev: ["vite"],
  start: ["scripts/start-dev.mjs"],
  stop: ["scripts/stop-dev.mjs"],
  test: ["npm run verify"],
  lint: ["vue-tsc", "cargo fmt"],
  build: ["vite build", "tauri build", "npm run web:build"],
  package: ["tauri build"],
  verify: ["scripts/health-check.mjs", "cargo test"],
  health: ["scripts/health-dev.mjs"],
};

const failures = [];

for (const name of requiredScripts) {
  const command = scripts[name];

  if (typeof command !== "string" || command.trim() === "") {
    failures.push(`${name}: missing`);
    continue;
  }

  if (/\becho\b|not wired|scaffold phase|No .* configured/i.test(command)) {
    failures.push(`${name}: placeholder command '${command}'`);
    continue;
  }

  const hints = realCommandHints[name] ?? [];
  if (!hints.some((hint) => command.includes(hint))) {
    failures.push(`${name}: expected one of ${hints.join(", ")}, got '${command}'`);
  }
}

if (failures.length > 0) {
  console.error(`desktop real script verification failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}

console.log("desktop real script verification ok");
