#!/usr/bin/env node
// Icon app của Ferry — máy bay giấy gấp ba mặt (cánh trên `#4D6BFE`, cánh dưới `#8FA6FF`, sống gấp `#2F49D1`) trên ô trắng,
// mặt gấp sau chừa khe 1,5 / 48 với mặt trước (chủ dự án chọn 11/11/2026: phương án "Tệp gấp thành máy bay", bỏ hai nét chữ trên
// cánh). Vẽ bằng mã (khoảng cách có dấu + khử răng cưa 4×4), không cần công cụ ảnh. Sinh lại (kết quả đã commit, build không gọi):
//   node packaging/icons/gen-icons.mjs
// Ra:
//   máy tính — packaging/linux/icons/<n>x<n>.png (hicolor), packaging/windows/ferry.ico, packaging/macos/ferry.icns,
//              packaging/icons/ferry.svg (bản vector gốc);
//   điện thoại — packaging/icons/mobile/android/res/… (mipmap thường + tròn + adaptive foreground / monochrome, màu nền),
//              packaging/icons/mobile/android/play-store-512.png, packaging/icons/mobile/ios/AppIcon.appiconset/…
//              (chép sang project app điện thoại; repo này không build app điện thoại).
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const WING = hex('#4D6BFE');
const LOWER = hex('#8FA6FF');
const KEEL = hex('#2F49D1');
const TILE_TOP = hex('#FFFFFF');
const TILE_BOTTOM = hex('#EEF1FF');
const WHITE = [255, 255, 255];

// Máy bay trong lưới 48 (mũi phải-trên). Thứ tự vẽ: cánh trên, cánh dưới, sống gấp; mặt vẽ sau khoét mặt trước một khe GAP.
const FACES = [
  { pts: [[42, 7], [6, 21], [18, 26]], rgb: WING },
  { pts: [[42, 7], [18, 26], [35, 35]], rgb: LOWER },
  { pts: [[42, 7], [18, 26], [21, 37], [26, 31]], rgb: KEEL },
];
const GAP = 1.5;
const PLANE_W = 36; // bề ngang máy bay (6..42)
const CENTER = [24, 22]; // tâm khung bao (6..42 × 7..37)

// Đa giác lồi: khoảng cách có dấu xấp xỉ bằng max các cạnh (âm = bên trong). Đỉnh tự đổi chiều cho pháp tuyến hướng ra.
const prepared = FACES.map(({ pts, rgb }) => {
  const area = pts.reduce((s, [ax, ay], i) => {
    const [bx, by] = pts[(i + 1) % pts.length];
    return s + ax * by - bx * ay;
  }, 0);
  const p = area < 0 ? [...pts].reverse() : pts;
  const edges = p.map(([ax, ay], i) => {
    const [bx, by] = p[(i + 1) % p.length];
    const len = Math.hypot(bx - ax, by - ay);
    return [ax, ay, (by - ay) / len, -(bx - ax) / len];
  });
  return { edges, rgb };
});
const dist = (face, x, y) => Math.max(...face.edges.map(([ax, ay, nx, ny]) => (x - ax) * nx + (y - ay) * ny));

// Màu máy bay tại điểm (đơn vị lưới 48), hoặc null nếu là nền / khe.
function planeAt(X, Y) {
  for (let i = prepared.length - 1; i >= 0; i--) {
    if (dist(prepared[i], X, Y) > 0) continue;
    const knocked = prepared.slice(i + 1).some((f) => dist(f, X, Y) < GAP);
    return knocked ? null : prepared[i].rgb;
  }
  return null;
}

function rrect(px, py, cx, cy, hw, hh, r) {
  const dx = Math.abs(px - cx) - (hw - r);
  const dy = Math.abs(py - cy) - (hh - r);
  return Math.hypot(Math.max(dx, 0), Math.max(dy, 0)) + Math.min(Math.max(dx, dy), 0) - r;
}

/// Một kiểu ảnh: `shape` ô nền ('rounded' | 'square' | 'circle' | 'none'), `margin` lề trong suốt quanh ô (0..0,5),
/// `plane` bề ngang máy bay so với cả ảnh, `mono` = chỉ dáng máy bay một màu (icon đơn sắc Android 13).
function sample(x, y, { shape, margin = 0, plane, mono = false }) {
  const half = 0.5 - margin;
  let inTile = true;
  if (shape === 'rounded') inTile = rrect(x, y, 0.5, 0.5, half, half, half * 0.45) <= 0;
  else if (shape === 'circle') inTile = Math.hypot(x - 0.5, y - 0.5) <= half;
  else if (shape === 'none') inTile = false;
  const unit = PLANE_W / plane; // đơn vị lưới 48 trên một bề ngang ảnh
  const c = planeAt(CENTER[0] + (x - 0.5) * unit, CENTER[1] + (y - 0.5) * unit);
  if (c) return [...(mono ? WHITE : c), 255];
  if (!inTile) return [0, 0, 0, 0];
  const t = Math.min(Math.max((y - margin) / (1 - 2 * margin), 0), 1);
  return [...TILE_TOP.map((v, i) => v + (TILE_BOTTOM[i] - v) * t), 255];
}

function render(size, opts) {
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
          const p = sample((x + (i + 0.5) / ss) / size, (y + (j + 0.5) / ss) / size, opts);
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
/// `opaque` ⇒ PNG RGB không kênh alpha (App Store từ chối icon iOS có alpha).
function png(size, opts, opaque = false) {
  const rgba = render(size, opts);
  const bpp = opaque ? 3 : 4;
  const raw = Buffer.alloc((size * bpp + 1) * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const s = (y * size + x) * 4;
      const d = y * (size * bpp + 1) + 1 + x * bpp;
      for (let k = 0; k < bpp; k++) raw[d + k] = rgba[s + k];
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = opaque ? 2 : 6;
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

// Kiểu ảnh theo nền tảng.
const DESKTOP = { shape: 'rounded', margin: 0.03, plane: 0.68 };
const MAC = { shape: 'rounded', margin: 0.1, plane: 0.55 }; // lưới macOS 11+: ô 824 / 1024
const IOS = { shape: 'square', plane: 0.66 }; // vuông kín, hệ thống tự bo
const ANDROID_LEGACY = { shape: 'rounded', margin: 0.06, plane: 0.62 };
const ANDROID_ROUND = { shape: 'circle', margin: 0.04, plane: 0.6 };
// Adaptive 108 dp: chỉ 66 dp giữa chắc chắn hiện ⇒ máy bay ~50 dp.
const ANDROID_FG = { shape: 'none', plane: 0.46 };
const ANDROID_MONO = { ...ANDROID_FG, mono: true };
const PLAY = { shape: 'square', plane: 0.66 };

const cache = new Map();
const get = (size, opts, opaque = false) => {
  const key = `${size}/${JSON.stringify(opts)}/${opaque}`;
  if (!cache.has(key)) cache.set(key, png(size, opts, opaque));
  return cache.get(key);
};
const write = (rel, data) => {
  const file = path.join(pkg, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data);
};

// Máy tính.
for (const size of [16, 24, 32, 48, 64, 128, 256, 512]) write(`linux/icons/${size}x${size}.png`, get(size, DESKTOP));
write('windows/ferry.ico', ico([16, 24, 32, 48, 64, 256].map((s) => [s, get(s, DESKTOP)])));
write('macos/ferry.icns', icns([
  ['ic11', get(32, MAC)], ['ic12', get(64, MAC)], ['ic07', get(128, MAC)], ['ic13', get(256, MAC)],
  ['ic08', get(256, MAC)], ['ic14', get(512, MAC)], ['ic09', get(512, MAC)], ['ic10', get(1024, MAC)],
]));

// Bản vector (cùng hình, khe bằng mask).
{
  const poly = (pts) => `M${pts.map((p) => p.join(' ')).join('L')}Z`;
  const masks = FACES.map((_, i) => {
    const later = FACES.slice(i + 1).map((f) => `<path d="${poly(f.pts)}" fill="#000" stroke="#000" stroke-width="${2 * GAP}" stroke-linejoin="round"/>`);
    return later.length ? `<mask id="k${i}" maskUnits="userSpaceOnUse" x="0" y="0" width="48" height="48"><rect width="48" height="48" fill="#fff"/>${later.join('')}</mask>` : '';
  }).join('');
  const faces = FACES.map((f, i) => {
    const fill = `#${f.rgb.map((v) => v.toString(16).padStart(2, '0')).join('')}`;
    const el = `<path d="${poly(f.pts)}" fill="${fill}"/>`;
    return i < FACES.length - 1 ? `<g mask="url(#k${i})">${el}</g>` : el;
  }).join('');
  write('icons/ferry.svg', `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
<defs><linearGradient id="t" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#EEF1FF"/></linearGradient>${masks}</defs>
<rect x="1.44" y="1.44" width="45.12" height="45.12" rx="10.15" fill="url(#t)"/>
<g transform="translate(24 24) scale(${(0.68 * 48) / PLANE_W}) translate(${-CENTER[0]} ${-CENTER[1]})">${faces}</g>
</svg>
`);
}

// Android.
const DENSITIES = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 };
for (const [d, k] of Object.entries(DENSITIES)) {
  write(`icons/mobile/android/res/mipmap-${d}/ic_launcher.png`, get(48 * k, ANDROID_LEGACY));
  write(`icons/mobile/android/res/mipmap-${d}/ic_launcher_round.png`, get(48 * k, ANDROID_ROUND));
  write(`icons/mobile/android/res/mipmap-${d}/ic_launcher_foreground.png`, get(108 * k, ANDROID_FG));
  write(`icons/mobile/android/res/mipmap-${d}/ic_launcher_monochrome.png`, get(108 * k, ANDROID_MONO));
}
const adaptive = `<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
    <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>
</adaptive-icon>
`;
write('icons/mobile/android/res/mipmap-anydpi-v26/ic_launcher.xml', adaptive);
write('icons/mobile/android/res/mipmap-anydpi-v26/ic_launcher_round.xml', adaptive);
write('icons/mobile/android/res/values/ic_launcher_background.xml', `<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#FFFFFF</color>
</resources>
`);
write('icons/mobile/android/play-store-512.png', get(512, PLAY, true));

// iOS: Xcode 14+ chỉ cần 1024 (một cỡ); kèm đủ cỡ cũ cho target iOS < 14 / Xcode cũ.
const IOS_SIZES = [
  ['iphone', 20, [2, 3]], ['iphone', 29, [2, 3]], ['iphone', 40, [2, 3]], ['iphone', 60, [2, 3]],
  ['ipad', 20, [1, 2]], ['ipad', 29, [1, 2]], ['ipad', 40, [1, 2]], ['ipad', 76, [1, 2]], ['ipad', 83.5, [2]],
  ['ios-marketing', 1024, [1]],
];
const images = [];
for (const [idiom, pt, scales] of IOS_SIZES) {
  for (const s of scales) {
    const px = Math.round(pt * s);
    const name = `icon-${px}.png`;
    write(`icons/mobile/ios/AppIcon.appiconset/${name}`, get(px, IOS, true));
    images.push({ filename: name, idiom, scale: `${s}x`, size: `${pt}x${pt}` });
  }
}
write('icons/mobile/ios/AppIcon.appiconset/Contents.json', `${JSON.stringify({ images, info: { author: 'xcode', version: 1 } }, null, 2)}\n`);

console.log('wrote linux/icons, windows/ferry.ico, macos/ferry.icns, icons/ferry.svg, icons/mobile/{android,ios}');
