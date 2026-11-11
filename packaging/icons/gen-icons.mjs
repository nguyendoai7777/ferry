#!/usr/bin/env node
// Icon app của Ferry (ferry-gd1_14) — vẽ bằng mã, không cần công cụ ảnh: ô vuông bo góc xanh Material `#005cbb` (primary của
// materiant), khung điện thoại trắng cùng dáng icon khay (`crates/daemon/src/tray.rs`), mũi tên vào màn hình = "gửi sang điện thoại".
// Sinh lại (kết quả đã commit, build không gọi script này):
//   node packaging/icons/gen-icons.mjs
// Ra: packaging/linux/icons/<n>x<n>.png (hicolor), packaging/windows/ferry.ico (PNG nhúng), packaging/macos/ferry.icns.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const TOP = [0x1e, 0x7b, 0xe6];
const BOTTOM = [0x00, 0x5c, 0xbb];
const WHITE = [0xff, 0xff, 0xff];

// Hình chữ nhật bo góc: khoảng cách có dấu (âm = bên trong). cx, cy tâm; hw, hh nửa cạnh; r bán kính góc.
function rrect(px, py, cx, cy, hw, hh, r) {
  const dx = Math.abs(px - cx) - (hw - r);
  const dy = Math.abs(py - cy) - (hh - r);
  return Math.hypot(Math.max(dx, 0), Math.max(dy, 0)) + Math.min(Math.max(dx, dy), 0) - r;
}

// Tam giác (mũi tên) qua ba đỉnh: âm = bên trong, xấp xỉ bằng max các cạnh (đủ cho khử răng cưa 4×4).
function triangle(px, py, pts) {
  let d = -Infinity;
  for (let i = 0; i < 3; i++) {
    const [ax, ay] = pts[i];
    const [bx, by] = pts[(i + 1) % 3];
    const nx = by - ay;
    const ny = -(bx - ax);
    const len = Math.hypot(nx, ny);
    d = Math.max(d, ((px - ax) * nx + (py - ay) * ny) / len);
  }
  return d;
}

// Một điểm ảnh (toạ độ 0..1) ⇒ [r, g, b, a] chưa khử răng cưa. `margin`: lề trong suốt quanh ô (macOS cần lề lớn hơn).
function sample(x, y, margin) {
  const half = 0.5 - margin;
  const bg = rrect(x, y, 0.5, 0.5, half, half, half * 0.45);
  if (bg > 0) return [0, 0, 0, 0];
  const t = (y - margin) / (1 - 2 * margin);
  let rgb = TOP.map((c, i) => c + (BOTTOM[i] - c) * t);
  const s = 1 - 2 * margin;
  // Khung điện thoại: rộng 0,40, cao 0,64 của ô, nét 0,055.
  const pw = 0.2 * s;
  const ph = 0.32 * s;
  const stroke = 0.055 * s;
  const cx = 0.5;
  const cy = 0.5;
  const frame = rrect(x, y, cx, cy, pw, ph, 0.07 * s);
  const inner = rrect(x, y, cx, cy, pw - stroke, ph - stroke, Math.max(0.07 * s - stroke, 0));
  const ring = frame <= 0 && inner > 0;
  // Chấm đáy.
  const dot = Math.hypot(x - cx, y - (cy + ph - stroke * 1.9)) <= stroke * 0.8;
  // Mũi tên xuống trong màn: thân + đầu tam giác.
  const shaftW = 0.045 * s;
  const shaft = rrect(x, y, cx, cy - 0.09 * s, shaftW, 0.09 * s, shaftW * 0.4) <= 0;
  const head = triangle(x, y, [[cx - 0.13 * s, cy - 0.02 * s], [cx + 0.13 * s, cy - 0.02 * s], [cx, cy + 0.13 * s]]) <= 0;
  if (ring || dot || shaft || head) rgb = WHITE;
  return [...rgb, 255];
}

function render(size, margin) {
  const out = Buffer.alloc(size * size * 4);
  const ss = 4;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      for (let j = 0; j < ss; j++) {
        for (let i = 0; i < ss; i++) {
          const p = sample((x + (i + 0.5) / ss) / size, (y + (j + 0.5) / ss) / size, margin);
          // Cộng dồn màu đã nhân alpha, chia lại sau: viền không bị đen.
          r += p[0] * p[3];
          g += p[1] * p[3];
          b += p[2] * p[3];
          a += p[3];
        }
      }
      const o = (y * size + x) * 4;
      if (a > 0) {
        out[o] = Math.round(r / a);
        out[o + 1] = Math.round(g / a);
        out[o + 2] = Math.round(b / a);
      }
      out[o + 3] = Math.round(a / (ss * ss));
    }
  }
  return out;
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}
function png(size, margin) {
  const rgba = render(size, margin);
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // 8 bit
  ihdr[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

function ico(images) {
  const head = Buffer.alloc(6 + 16 * images.length);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(images.length, 4);
  let offset = head.length;
  images.forEach(([size, data], i) => {
    const e = 6 + 16 * i;
    head[e] = size >= 256 ? 0 : size;
    head[e + 1] = size >= 256 ? 0 : size;
    head.writeUInt16LE(1, e + 4);
    head.writeUInt16LE(32, e + 6);
    head.writeUInt32LE(data.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += data.length;
  });
  return Buffer.concat([head, ...images.map(([, d]) => d)]);
}

function icns(entries) {
  const parts = entries.map(([type, data]) => {
    const h = Buffer.alloc(8);
    h.write(type, 0, 'ascii');
    h.writeUInt32BE(data.length + 8, 4);
    return Buffer.concat([h, data]);
  });
  const body = Buffer.concat(parts);
  const h = Buffer.alloc(8);
  h.write('icns', 0, 'ascii');
  h.writeUInt32BE(body.length + 8, 4);
  return Buffer.concat([h, body]);
}

const FLAT = 0.03;
// Lưới icon macOS 11+: ô 824 / 1024 ⇒ lề ~0,1.
const MAC = 0.1;
const cache = new Map();
const get = (size, margin) => {
  const key = `${size}/${margin}`;
  if (!cache.has(key)) cache.set(key, png(size, margin));
  return cache.get(key);
};

for (const size of [16, 24, 32, 48, 64, 128, 256, 512]) {
  fs.writeFileSync(path.join(pkg, `linux/icons/${size}x${size}.png`), get(size, FLAT));
}
fs.writeFileSync(path.join(pkg, 'windows/ferry.ico'), ico([16, 24, 32, 48, 64, 256].map((s) => [s, get(s, FLAT)])));
fs.writeFileSync(path.join(pkg, 'macos/ferry.icns'), icns([
  ['ic11', get(32, MAC)], ['ic12', get(64, MAC)], ['ic07', get(128, MAC)], ['ic13', get(256, MAC)],
  ['ic08', get(256, MAC)], ['ic14', get(512, MAC)], ['ic09', get(512, MAC)], ['ic10', get(1024, MAC)],
]));
console.log('wrote linux/icons, windows/ferry.ico, macos/ferry.icns');
