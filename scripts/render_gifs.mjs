#!/usr/bin/env node
/**
 * Offline frame-by-frame TUI GIF recorder & renderer for CqShell.
 *
 * One-step usage (records real Ratatui frames via `cargo test` and writes GIFs to `assets/demos/`):
 *   node scripts/render_gifs.mjs
 *
 * Advanced usage:
 *   node scripts/render_gifs.mjs [--frames-dir <dir>] [--out-dir <dir>] [--no-record] [--keep-temp]
 *
 * Requirements:
 *   - Node.js 22+ (built-in WebSocket & fetch)
 *   - Chrome / Chromium / Edge (override via CQS_CHROME_BIN or CHROME_PATH)
 *   - ffmpeg on PATH
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';

const REPO_ROOT = path.resolve(import.meta.dirname, '..');

function parseArgs(argv) {
  const opts = {
    framesDir: null,
    outDir: path.join(REPO_ROOT, 'assets', 'demos'),
    record: true,
    keepTemp: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--frames-dir' && argv[i + 1]) {
      opts.framesDir = path.resolve(argv[++i]);
    } else if (arg === '--out-dir' && argv[i + 1]) {
      opts.outDir = path.resolve(argv[++i]);
    } else if (arg === '--no-record') {
      opts.record = false;
    } else if (arg === '--keep-temp') {
      opts.keepTemp = true;
    } else if (arg === '-h' || arg === '--help') {
      console.log(
        'Usage: node scripts/render_gifs.mjs [--frames-dir <dir>] [--out-dir <dir>] [--no-record] [--keep-temp]',
      );
      process.exit(0);
    } else {
      console.error(`Unknown argument: ${arg}`);
      process.exit(1);
    }
  }
  return opts;
}

function findChromeBinary() {
  for (const envKey of ['CQS_CHROME_BIN', 'CHROME_PATH']) {
    const val = process.env[envKey];
    if (val && fs.existsSync(val)) return val;
  }

  const candidates =
    process.platform === 'win32'
      ? [
          path.join(process.env.PROGRAMFILES || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
          path.join(process.env['PROGRAMFILES(X86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
          path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
          path.join(process.env['PROGRAMFILES(X86)'] || 'C:\\Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
          path.join(process.env.PROGRAMFILES || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
        ]
      : process.platform === 'darwin'
        ? [
            '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
            '/Applications/Chromium.app/Contents/MacOS/Chromium',
            '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
          ]
        : [
            '/usr/bin/google-chrome',
            '/usr/bin/google-chrome-stable',
            '/usr/bin/chromium',
            '/usr/bin/chromium-browser',
          ];

  for (const p of candidates) {
    if (p && fs.existsSync(p)) return p;
  }

  for (const name of ['google-chrome', 'chromium', 'chromium-browser', 'chrome', 'msedge']) {
    const res = spawnSync(process.platform === 'win32' ? 'where' : 'which', [name], { encoding: 'utf8' });
    if (res.status === 0 && res.stdout.trim()) {
      return res.stdout.trim().split(/\r?\n/)[0];
    }
  }

  throw new Error('Could not find Chrome/Chromium/Edge. Set CQS_CHROME_BIN to the browser executable.');
}

const RENDERER_HTML = `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
  html, body { margin: 0; padding: 0; background: #000; overflow: hidden; }
  canvas { display: block; }
</style>
</head>
<body>
<canvas id="c"></canvas>
<script>
const canvas = document.getElementById('c');
const ctx = canvas.getContext('2d', { alpha: false });

const CELL_W = 9;
const CELL_H = 19;
const FONT_MONO = '"Cascadia Mono", "Cascadia Code", "JetBrains Mono", "DejaVu Sans Mono", Consolas, monospace';
const FONT_CJK  = '"Microsoft YaHei UI", "PingFang SC", "Noto Sans CJK SC", "Noto Sans SC", sans-serif';

function parseHex(hex) {
  const h = hex.replace('#', '');
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}

function luma(hex) {
  const [r, g, b] = parseHex(hex);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function mixHex(a, b, t) {
  const [r1, g1, b1] = parseHex(a);
  const [r2, g2, b2] = parseHex(b);
  const r = Math.round(r1 + (r2 - r1) * t);
  const g = Math.round(g1 + (g2 - g1) * t);
  const bl = Math.round(b1 + (b2 - b1) * t);
  return '#' + [r, g, bl].map(v => v.toString(16).padStart(2, '0')).join('');
}

function drawRoundedRect(x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

function drawVectorGlyph(sym, px, py, fg) {
  const cp = sym.codePointAt(0);
  if (cp === undefined) return false;

  if (cp >= 0x2800 && cp <= 0x28ff) {
    const mask = cp - 0x2800;
    if (mask === 0) return true;
    ctx.fillStyle = fg;
    const cols = [px + 1.5, px + 5.2];
    const rows = [py + 2.0, py + 6.2, py + 10.4, py + 14.6];
    const dotW = 2.4;
    const dotH = 2.6;
    const bits = [
      [0x01, 0, 0], [0x02, 0, 1], [0x04, 0, 2], [0x40, 0, 3],
      [0x08, 1, 0], [0x10, 1, 1], [0x20, 1, 2], [0x80, 1, 3],
    ];
    for (const [b, c, r] of bits) {
      if (mask & b) {
        ctx.fillRect(Math.round(cols[c]), Math.round(rows[r]), Math.round(dotW), Math.round(dotH));
      }
    }
    return true;
  }

  if (cp >= 0x2580 && cp <= 0x259f) {
    ctx.fillStyle = fg;
    if (cp === 0x2588) { ctx.fillRect(px, py, CELL_W, CELL_H); return true; }
    if (cp === 0x2580) { ctx.fillRect(px, py, CELL_W, Math.floor(CELL_H / 2)); return true; }
    if (cp >= 0x2581 && cp <= 0x2587) {
      const bh = Math.max(2, Math.round(CELL_H * ((cp - 0x2580) / 8)));
      ctx.fillRect(px, py + CELL_H - bh, CELL_W, bh);
      return true;
    }
    if (cp >= 0x2589 && cp <= 0x258f) {
      const bw = Math.max(1, Math.round(CELL_W * ((0x2590 - cp) / 8)));
      ctx.fillRect(px, py, bw, CELL_H);
      return true;
    }
    if (cp === 0x2590) {
      const bw = Math.floor(CELL_W / 2);
      ctx.fillRect(px + CELL_W - bw, py, bw, CELL_H);
      return true;
    }
    if (cp === 0x2594) { ctx.fillRect(px, py, CELL_W, 2); return true; }
    if (cp === 0x2595) { ctx.fillRect(px + CELL_W - 2, py, 2, CELL_H); return true; }
    if (cp === 0x2591 || cp === 0x2592 || cp === 0x2593) {
      ctx.save();
      ctx.globalAlpha = cp === 0x2591 ? 0.22 : cp === 0x2592 ? 0.45 : 0.72;
      ctx.fillRect(px, py, CELL_W, CELL_H);
      ctx.restore();
      return true;
    }
    return false;
  }

  if (cp >= 0x2500 && cp <= 0x257f) {
    const cx = px + Math.floor(CELL_W / 2) + 0.5;
    const cy = py + Math.floor(CELL_H / 2) + 0.5;
    const x0 = px;
    const x1 = px + CELL_W;
    const y0 = py;
    const y1 = py + CELL_H;

    ctx.strokeStyle = fg;
    ctx.fillStyle = fg;
    ctx.lineCap = 'butt';
    ctx.lineJoin = 'miter';

    if (cp === 0x256d || cp === 0x256e || cp === 0x256f || cp === 0x2570) {
      ctx.lineWidth = 1.25;
      ctx.beginPath();
      if (cp === 0x256d) { ctx.moveTo(x1, cy); ctx.quadraticCurveTo(cx, cy, cx, y1); }
      else if (cp === 0x256e) { ctx.moveTo(x0, cy); ctx.quadraticCurveTo(cx, cy, cx, y1); }
      else if (cp === 0x256f) { ctx.moveTo(x0, cy); ctx.quadraticCurveTo(cx, cy, cx, y0); }
      else { ctx.moveTo(x1, cy); ctx.quadraticCurveTo(cx, cy, cx, y0); }
      ctx.stroke();
      return true;
    }

    if (cp === 0x2504 || cp === 0x2508 || cp === 0x254c) {
      ctx.fillRect(x0 + 1, Math.floor(cy), 3, 1);
      ctx.fillRect(x0 + 5, Math.floor(cy), 3, 1);
      return true;
    }
    if (cp === 0x2506 || cp === 0x250a || cp === 0x254e) {
      ctx.fillRect(Math.floor(cx), y0 + 2, 1, 4);
      ctx.fillRect(Math.floor(cx), y0 + 10, 1, 4);
      return true;
    }

    const segs = {
      0x2500: [1,1,0,0], 0x2501: [2,2,0,0],
      0x2502: [0,0,1,1], 0x2503: [0,0,2,2],
      0x250c: [0,1,0,1], 0x250f: [0,2,0,2],
      0x2510: [1,0,0,1], 0x2513: [2,0,0,2],
      0x2514: [0,1,1,0], 0x2517: [0,2,2,0],
      0x2518: [1,0,1,0], 0x251b: [2,0,2,0],
      0x251c: [0,1,1,1], 0x2523: [0,2,2,2],
      0x2524: [1,0,1,1], 0x252b: [2,0,2,2],
      0x252c: [1,1,0,1], 0x2533: [2,2,0,2],
      0x2534: [1,1,1,0], 0x253b: [2,2,2,0],
      0x253c: [1,1,1,1], 0x254b: [2,2,2,2],
      0x2574: [1,0,0,0], 0x2575: [0,0,1,0],
      0x2576: [0,1,0,0], 0x2577: [0,0,0,1],
      0x2578: [2,0,0,0], 0x2579: [0,0,2,0],
      0x257a: [0,2,0,0], 0x257b: [0,0,0,2],
      0x257c: [1,2,0,0], 0x257d: [0,0,1,2],
      0x257e: [2,1,0,0], 0x257f: [0,0,2,1],
    }[cp];

    if (segs) {
      const [l, r, u, d] = segs;
      const drawArm = (xA, yA, xB, yB, weight) => {
        if (!weight) return;
        ctx.lineWidth = weight === 2 ? 2.2 : 1.1;
        ctx.beginPath();
        ctx.moveTo(xA, yA);
        ctx.lineTo(xB, yB);
        ctx.stroke();
      };
      const maxW = Math.max(l, r, u, d) === 2 ? 1.1 : 0.55;
      if (l) drawArm(x0, cy, cx + (r ? 0 : maxW), cy, l);
      if (r) drawArm(cx - (l ? 0 : maxW), cy, x1, cy, r);
      if (u) drawArm(cx, y0, cx, cy + (d ? 0 : maxW), u);
      if (d) drawArm(cx, cy - (u ? 0 : maxW), cx, y1, d);
      return true;
    }
    return false;
  }

  const cx = px + CELL_W / 2;
  const cy = py + CELL_H / 2;
  ctx.fillStyle = fg;
  ctx.strokeStyle = fg;
  if (cp === 0x25a0 || cp === 0x25aa) {
    const s = cp === 0x25a0 ? 6 : 4;
    ctx.fillRect(Math.round(cx - s / 2), Math.round(cy - s / 2), s, s);
    return true;
  }
  if (cp === 0x25a1 || cp === 0x25ab) {
    const s = cp === 0x25a1 ? 6 : 4;
    ctx.lineWidth = 1;
    ctx.strokeRect(Math.round(cx - s / 2) + 0.5, Math.round(cy - s / 2) + 0.5, s - 1, s - 1);
    return true;
  }
  if (cp === 0x25c6) {
    ctx.beginPath();
    ctx.moveTo(cx, cy - 3.6); ctx.lineTo(cx + 3.4, cy);
    ctx.lineTo(cx, cy + 3.6); ctx.lineTo(cx - 3.4, cy);
    ctx.closePath(); ctx.fill();
    return true;
  }
  if (cp === 0x25c7) {
    ctx.lineWidth = 1.1;
    ctx.beginPath();
    ctx.moveTo(cx, cy - 3.5); ctx.lineTo(cx + 3.3, cy);
    ctx.lineTo(cx, cy + 3.5); ctx.lineTo(cx - 3.3, cy);
    ctx.closePath(); ctx.stroke();
    return true;
  }
  if (cp === 0x25cf) { ctx.beginPath(); ctx.arc(cx, cy, 3.0, 0, Math.PI * 2); ctx.fill(); return true; }
  if (cp === 0x25cb) { ctx.lineWidth = 1.1; ctx.beginPath(); ctx.arc(cx, cy, 2.8, 0, Math.PI * 2); ctx.stroke(); return true; }
  if (cp === 0x2022 || cp === 0x00b7) {
    ctx.beginPath(); ctx.arc(cx, cy, cp === 0x2022 ? 2.1 : 1.35, 0, Math.PI * 2); ctx.fill();
    return true;
  }
  return false;
}

window.renderFrame = function(frame) {
  const { w, h, bg, fg, mobile, title, caption, key, cells } = frame;
  const termW = w * CELL_W;
  const termH = h * CELL_H;
  const padX = 14;
  const padBottom = mobile ? 40 : 14;
  const barH = 36;

  const totalW = termW + padX * 2;
  const totalH = termH + barH + padBottom;

  if (canvas.width !== totalW || canvas.height !== totalH) {
    canvas.width = totalW;
    canvas.height = totalH;
  }

  const isLight = luma(bg) > 140;
  const chromeBg = isLight ? mixHex(bg, '#dde3ec', 0.55) : mixHex(bg, '#04060a', 0.35);
  const borderColor = isLight ? mixHex(bg, '#94a3b8', 0.5) : mixHex(bg, '#475569', 0.42);
  const titleColor = isLight ? '#334155' : '#94a3b8';
  const pillBg = isLight ? mixHex(bg, '#cbd5e1', 0.5) : mixHex(bg, '#334155', 0.45);
  const pillText = isLight ? '#0f172a' : '#e2e8f0';
  const keyBg = isLight ? '#0f172a' : '#38bdf8';
  const keyText = isLight ? '#f8fafc' : '#0b1120';

  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, totalW, totalH);

  ctx.fillStyle = chromeBg;
  ctx.fillRect(0, 0, totalW, barH);
  ctx.fillStyle = borderColor;
  ctx.fillRect(0, barH - 1, totalW, 1);

  if (mobile) {
    ctx.font = '600 12px "Segoe UI", "PingFang SC", "Microsoft YaHei UI", sans-serif';
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'left';
    ctx.fillStyle = pillText;
    ctx.fillText('9:41', 16, barH / 2 + 0.5);

    ctx.textAlign = 'center';
    ctx.fillStyle = titleColor;
    ctx.fillText(title || 'Termius · cqs', totalW / 2, barH / 2 + 0.5);

    const bx = totalW - 38;
    const by = Math.floor(barH / 2 - 5);
    ctx.strokeStyle = titleColor;
    ctx.lineWidth = 1.2;
    drawRoundedRect(bx, by, 20, 10, 2.5);
    ctx.stroke();
    ctx.fillStyle = '#4ade80';
    ctx.fillRect(bx + 2, by + 2, 14, 6);
    ctx.fillStyle = titleColor;
    ctx.fillRect(bx + 21, by + 3, 2, 4);

    ctx.fillStyle = borderColor;
    drawRoundedRect(totalW / 2 - 56, totalH - 10, 112, 4, 2);
    ctx.fill();
  } else {
    const dotY = Math.floor(barH / 2);
    const dots = ['#ff5f56', '#ffbd2e', '#27c93f'];
    for (let i = 0; i < 3; i++) {
      ctx.beginPath();
      ctx.arc(20 + i * 18, dotY, 5.5, 0, Math.PI * 2);
      ctx.fillStyle = dots[i];
      ctx.fill();
    }

    ctx.font = '500 12.5px "Segoe UI", "PingFang SC", "Microsoft YaHei UI", sans-serif';
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'left';
    ctx.fillStyle = titleColor;
    ctx.fillText(title || 'cqs', 76, dotY + 0.5);
  }

  if (caption && !mobile) {
    ctx.font = '600 12px "Microsoft YaHei UI", "PingFang SC", "Noto Sans SC", sans-serif';
    const capW = ctx.measureText(caption).width;
    let rightX = totalW - 14;

    if (key) {
      ctx.font = '700 11.5px ' + FONT_MONO;
      const kw = Math.max(22, ctx.measureText(key).width + 14);
      const kx = rightX - kw;
      ctx.fillStyle = keyBg;
      drawRoundedRect(kx, 8, kw, 20, 5);
      ctx.fill();
      ctx.fillStyle = keyText;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(key, kx + kw / 2, 18.5);
      rightX = kx - 8;
    }

    const pw = capW + 18;
    const px = rightX - pw;
    ctx.fillStyle = pillBg;
    drawRoundedRect(px, 8, pw, 20, 5);
    ctx.fill();
    ctx.font = '500 12px "Microsoft YaHei UI", "PingFang SC", "Noto Sans SC", sans-serif';
    ctx.fillStyle = pillText;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(caption, px + pw / 2, 18.5);
  } else if (caption && mobile) {
    ctx.font = '500 11.5px "Microsoft YaHei UI", "PingFang SC", "Noto Sans SC", sans-serif';
    ctx.fillStyle = titleColor;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    const label = key ? (caption + '  [' + key + ']') : caption;
    ctx.fillText(label, totalW / 2, totalH - 23);
  }

  const ox = padX;
  const oy = barH + 4;

  for (let i = 0; i < cells.length; i++) {
    const [x, y, span, sym, cfg, cbg] = cells[i];
    if (cbg && cbg !== bg) {
      ctx.fillStyle = cbg;
      ctx.fillRect(ox + x * CELL_W, oy + y * CELL_H, span * CELL_W, CELL_H);
    }
  }

  ctx.textBaseline = 'middle';
  for (let i = 0; i < cells.length; i++) {
    const [x, y, span, sym, cfg, cbg, flags] = cells[i];
    if (!sym || sym === ' ') continue;
    const px = ox + x * CELL_W;
    const py = oy + y * CELL_H;
    const color = cfg || fg;

    if (span === 1 && sym.length === 1 && drawVectorGlyph(sym, px, py, color)) {
      continue;
    }

    const bold = (flags & 1) ? '700 ' : '400 ';
    const italic = (flags & 4) ? 'italic ' : '';
    ctx.fillStyle = color;

    if (span >= 2) {
      ctx.font = italic + bold + '13.5px ' + FONT_CJK;
      ctx.textAlign = 'center';
      ctx.fillText(sym, px + (span * CELL_W) / 2, py + CELL_H / 2 + 0.5, span * CELL_W - 0.5);
    } else {
      const cp = sym.codePointAt(0);
      const isAscii = cp < 0x80;
      ctx.font = italic + bold + (isAscii ? '14.5px ' : '14px ') + FONT_MONO + ', ' + FONT_CJK;
      ctx.textAlign = 'center';
      ctx.fillText(sym, px + CELL_W / 2, py + CELL_H / 2 + 0.5, CELL_W + 0.5);
    }

    if (flags & 8) {
      ctx.fillStyle = color;
      ctx.fillRect(px, py + CELL_H - 2, span * CELL_W, 1);
    }
  }

  ctx.strokeStyle = borderColor;
  ctx.lineWidth = 1;
  ctx.strokeRect(0.5, 0.5, totalW - 1, totalH - 1);

  return canvas.toDataURL('image/png').slice('data:image/png;base64,'.length);
};
</script>
</body>
</html>`;

async function startChrome(chromePath, htmlPath, workDir) {
  const userDataDir = path.join(workDir, '_chrome_profile');
  fs.mkdirSync(userDataDir, { recursive: true });
  const port = 19222 + Math.floor(Math.random() * 500);
  const proc = spawn(
    chromePath,
    [
      '--headless=new',
      '--disable-gpu',
      '--no-sandbox',
      '--disable-dev-shm-usage',
      '--disable-extensions',
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${userDataDir}`,
      pathToFileURL(htmlPath).href,
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );

  let wsUrl = null;
  for (let i = 0; i < 60; i++) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/list`);
      const targets = await res.json();
      const page = targets.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
      if (page) {
        wsUrl = page.webSocketDebuggerUrl;
        break;
      }
    } catch {}
    await new Promise((r) => setTimeout(r, 150));
  }
  if (!wsUrl) {
    proc.kill();
    throw new Error('Failed to connect to headless Chrome CDP');
  }
  return { proc, wsUrl, userDataDir };
}

class CdpClient {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.ready = new Promise((resolve, reject) => {
      this.ws.addEventListener('open', resolve);
      this.ws.addEventListener('error', reject);
    });
    this.ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data.toString());
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(msg.error.message));
        else resolve(msg.result);
      }
    });
  }
  async send(method, params = {}) {
    await this.ready;
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  close() {
    try {
      this.ws.close();
    } catch {}
  }
}

async function renderScene(cdp, workDir, outDir, ndjsonPath, sceneName) {
  const lines = fs.readFileSync(ndjsonPath, 'utf8').trim().split(/\r?\n/).filter(Boolean);
  const pngDir = path.join(workDir, `_png_${sceneName}`);
  fs.rmSync(pngDir, { recursive: true, force: true });
  fs.mkdirSync(pngDir, { recursive: true });

  const concatLines = ['ffconcat version 1.0'];
  let totalMs = 0;
  let lastPng = null;

  for (let i = 0; i < lines.length; i++) {
    const frameJson = lines[i];
    const parsed = JSON.parse(frameJson);
    const delaySec = Math.max(0.04, (parsed.delay_ms || 80) / 1000);
    totalMs += parsed.delay_ms || 80;

    const evalRes = await cdp.send('Runtime.evaluate', {
      expression: `window.renderFrame(${frameJson})`,
      returnByValue: true,
    });
    if (evalRes.exceptionDetails) {
      throw new Error(
        `Canvas render error on ${sceneName}#${i}: ${JSON.stringify(evalRes.exceptionDetails)}`,
      );
    }
    const b64 = evalRes.result.value;
    const pngName = `f_${String(i).padStart(4, '0')}.png`;
    fs.writeFileSync(path.join(pngDir, pngName), Buffer.from(b64, 'base64'));
    concatLines.push(`file '${pngName}'`);
    concatLines.push(`duration ${delaySec.toFixed(3)}`);
    lastPng = pngName;
  }

  if (lastPng) {
    concatLines.push(`file '${lastPng}'`);
  }

  const concatPath = path.join(pngDir, 'frames.txt');
  fs.writeFileSync(concatPath, concatLines.join('\n') + '\n', 'utf8');

  const palettePath = path.join(pngDir, 'palette.png');
  const outGif = path.join(outDir, `${sceneName}.gif`);

  const p1 = spawnSync(
    'ffmpeg',
    [
      '-y', '-loglevel', 'error',
      '-f', 'concat', '-safe', '0', '-i', concatPath,
      '-vf', 'palettegen=max_colors=256:stats_mode=diff',
      palettePath,
    ],
    { stdio: 'inherit' },
  );
  if (p1.status !== 0) throw new Error(`ffmpeg palettegen failed for ${sceneName}`);

  const p2 = spawnSync(
    'ffmpeg',
    [
      '-y', '-loglevel', 'error',
      '-f', 'concat', '-safe', '0', '-i', concatPath,
      '-i', palettePath,
      '-lavfi', 'paletteuse=dither=none:diff_mode=rectangle',
      '-loop', '0',
      outGif,
    ],
    { stdio: 'inherit' },
  );
  if (p2.status !== 0) throw new Error(`ffmpeg paletteuse failed for ${sceneName}`);

  const bytes = fs.statSync(outGif).size;
  console.log(
    `Rendered ${sceneName}.gif: ${lines.length} frames, ${(totalMs / 1000).toFixed(1)}s, ${(bytes / 1024).toFixed(1)} KB`,
  );
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const chromePath = findChromeBinary();
  const ownedTemp = !opts.framesDir;
  const workDir = opts.framesDir || fs.mkdtempSync(path.join(os.tmpdir(), 'cqs-gif-'));
  fs.mkdirSync(workDir, { recursive: true });
  fs.mkdirSync(opts.outDir, { recursive: true });

  try {
    if (opts.record) {
      const rec = spawnSync(
        'cargo',
        ['test', '--locked', '--lib', '--', '--ignored', 'record_readme_gifs', '--nocapture'],
        {
          cwd: REPO_ROOT,
          env: { ...process.env, CQS_GIF_OUT_DIR: workDir },
          stdio: 'inherit',
        },
      );
      if (rec.status !== 0) {
        throw new Error(`cargo test record_readme_gifs failed with status ${rec.status}`);
      }
    }

    const htmlPath = path.join(workDir, '_renderer.html');
    fs.writeFileSync(htmlPath, RENDERER_HTML, 'utf8');

    const { proc, wsUrl, userDataDir } = await startChrome(chromePath, htmlPath, workDir);
    const cdp = new CdpClient(wsUrl);
    try {
      await cdp.send('Runtime.evaluate', {
        expression: 'document.fonts.ready.then(() => typeof window.renderFrame)',
        awaitPromise: true,
      });

      const scenes = fs
        .readdirSync(workDir)
        .filter((f) => f.endsWith('.ndjson'))
        .sort();
      if (scenes.length === 0) {
        throw new Error(`No .ndjson frame files found in ${workDir}`);
      }

      for (const file of scenes) {
        const name = file.replace(/\.ndjson$/, '');
        await renderScene(cdp, workDir, opts.outDir, path.join(workDir, file), name);
      }
    } finally {
      cdp.close();
      proc.kill();
      try {
        fs.rmSync(userDataDir, { recursive: true, force: true });
      } catch {}
    }
  } finally {
    if (ownedTemp && !opts.keepTemp) {
      try {
        fs.rmSync(workDir, { recursive: true, force: true });
      } catch {}
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
