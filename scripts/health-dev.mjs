const url = process.env.WT_MEDIA_DESKTOP_DEV_URL ?? "http://127.0.0.1:5174/";

try {
  const response = await fetch(url);
  const body = await response.text();

  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }

  if (!body.includes("WT Media Desktop") || !body.includes("/src/main.ts")) {
    throw new Error("unexpected desktop dev page response");
  }

  console.log(`wt-media-desktop dev health ok: ${url}`);
} catch (error) {
  console.error(`wt-media-desktop dev health failed: ${url}: ${error.message}`);
  process.exit(1);
}
