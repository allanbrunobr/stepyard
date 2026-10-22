// Render an asciicast into a GIF with Playwright: asciinema-player draws the
// terminal in Chrome, each frame is a screenshot, gifenc encodes the result.
// Usage (from scripts/demo/): node render.mjs demo.cast ../../docs/assets/demo.gif
import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import gifenc from 'gifenc';
import fs from 'fs';
import http from 'http';
import path from 'path';
const { GIFEncoder, quantize, applyPalette } = gifenc;
const FPS = 10;
const castPath = process.argv[2] || 'demo.cast';
const out = process.argv[3] || 'demo.gif';
const lines = fs.readFileSync(castPath,'utf8').trim().split('\n');
const duration = JSON.parse(lines[lines.length-1])[0];
const server = http.createServer((req, res) => {
  const f = path.join(process.cwd(), decodeURIComponent(req.url.split('?')[0]));
  fs.readFile(f, (err, data) => { if (err) { res.writeHead(404); res.end(); } else { res.end(data); } });
}).listen(0);
const port = server.address().port;
const browser = await chromium.launch({ channel: 'chrome' });
const page = await browser.newPage({ deviceScaleFactor: 1 });
await page.goto(`http://127.0.0.1:${port}/render.html?cast=${encodeURIComponent(castPath)}`);
await page.waitForFunction(() => window.player && document.querySelector('.ap-term'));
await page.evaluate(() => window.player.seek(0.01));
await page.waitForTimeout(500);
const el = await page.$('#wrap');
const frames = [];
for (let i = 0; i <= Math.ceil(duration * FPS); i++) {
  const t = Math.min(i / FPS, duration);
  await page.evaluate(async (t) => { await window.player.seek(t); }, t);
  await page.waitForTimeout(15);
  frames.push(await el.screenshot());
}
await browser.close();
server.close();
const decoded = frames.map(b => PNG.sync.read(b));
const { width, height } = decoded[0];
// one global palette (sampled across the run) keeps colors stable
const sample = new Uint8Array(width * height * 4 * 6);
[0.1,0.3,0.5,0.6,0.8,0.99].forEach((f,k)=> sample.set(decoded[Math.floor(f*(decoded.length-1))].data, k*width*height*4));
const palette = quantize(sample, 64);
const gif = GIFEncoder();
let prev = null, delay = 0, pending = null;
const flush = () => { if (pending) gif.writeFrame(pending, width, height, { palette, delay }); };
for (const d of decoded) {
  const idx = applyPalette(d.data, palette);
  if (prev && Buffer.compare(Buffer.from(idx), Buffer.from(prev)) === 0) { delay += 1000/FPS; continue; }
  flush(); pending = idx; prev = idx; delay = 1000/FPS;
}
delay += 2500; flush();
gif.finish();
fs.writeFileSync(out, gif.bytes());
console.log(out, width+'x'+height, frames.length+' frames', (gif.bytes().length/1024).toFixed(0)+' KB');
