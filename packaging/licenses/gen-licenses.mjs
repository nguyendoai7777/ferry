#!/usr/bin/env node
// Trang giấy phép bên thứ ba của Ferry (ferry-gd1_14, bê `groove/packaging/licenses`): một bản Node chạy cả ba OS (máy nào cũng có Node).
// Sinh `<Out>/third-party.html`: cây phụ thuộc của 6 target phát hành (`about.toml`, cùng danh sách `deny.toml`), gộp theo
// giấy phép, đủ văn bản, cộng thứ cargo không thấy (mảng `extra` dưới đây). `cargo deny check licenses` đỏ ⇒ dừng.
//   cargo install cargo-about --locked --features cli
//   node packaging/licenses/gen-licenses.mjs dist/licenses
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const out = process.argv[2];
if (!out || process.argv.length !== 3) fail('cách dùng: node packaging/licenses/gen-licenses.mjs <thư mục ra>');

const dir = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(dir, '../..');

// Thứ cargo không thấy: icon Lucide và font Mona Sans nhúng qua materiant (feature `font-mona-sans`, gd1_12), SQLite biên dịch
// kèm `rusqlite` (`bundled`, công cộng).
const extra = [
  {
    name: 'SQLite', version: 'compiled into ferry (rusqlite, bundled)', license: 'blessing',
    url: 'https://sqlite.org/copyright.html', text: 'sqlite.txt',
  },
  {
    name: 'Lucide', version: 'icons used by Ferry and the materiant library', license: 'ISC',
    url: 'https://lucide.dev', text: '../../../materiant/assets/icons/LICENSE-lucide',
  },
  {
    name: 'Mona Sans', version: 'font embedded by the materiant library', license: 'OFL-1.1',
    url: 'https://github.com/github/mona-sans', text: '../../../materiant/assets/fonts/mona-sans/OFL.txt',
  },
];
// Không thành phần LGPL nào được chấp nhận ở ferry (D-2).
const lgplAllowed = null;
// Thứ phải có: một target rơi khỏi cây thì tên của nó biến mất (tray-icon: Windows / macOS; zbus: Linux; objc2: macOS).
const required = ['gpui-pre', 'gpui-kit', 'reqwest', 'rusqlite', 'mdns-sd', 'keyring', 'notify-rust', 'ureq', 'tray-icon', 'zbus', 'windows-sys', 'objc2'];

function fail(message) {
  console.error(`gen-licenses: ${message}`);
  process.exit(1);
}

function run(args) {
  const r = spawnSync('cargo', args, { cwd: repo, stdio: 'inherit' });
  if (r.status !== 0) fail(`cargo ${args[0]} failed (exit ${r.status})`);
}

function targets(file) {
  const block = /^targets\s*=\s*\[([\s\S]*?)^\]/m.exec(fs.readFileSync(file, 'utf8'));
  if (!block) fail(`no targets in ${file}`);
  return [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]).sort();
}

// Như `System.Net.WebUtility.HtmlEncode` của bản pwsh: thêm cả U+00A0–U+00FF dạng số, để hai bản ra cùng một tệp.
function escapeHtml(text) {
  return text.replace(/[&<>"' -ÿ]/g, (ch) => {
    switch (ch) {
      case '&': return '&amp;';
      case '<': return '&lt;';
      case '>': return '&gt;';
      case '"': return '&quot;';
      case "'": return '&#39;';
      default: return `&#${ch.charCodeAt(0)};`;
    }
  });
}

function decodeHtml(text) {
  return text
    .replace(/&#(\d+);/g, (_, n) => String.fromCharCode(Number(n)))
    .replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
}

const readText = (file) => fs.readFileSync(file, 'utf8').replace(/\r\n/g, '\n');
const unique = (items) => [...new Set(items)].sort();

// Hai danh sách target lệch nhau ⇒ trang giấy phép không còn khớp thứ `cargo deny` đã soát.
if (targets(path.join(dir, 'about.toml')).join(',') !== targets(path.join(repo, 'deny.toml')).join(',')) {
  fail('targets differ: about.toml vs deny.toml');
}

// `--frozen`: không gọi ClearlyDefined, chỉ đọc nguồn crate đã tải, chạy lại ra cùng nội dung.
run(['fetch', '--locked']);
run(['deny', '--log-level', 'error', 'check', 'licenses']);
// Tệp tạm trong thư mục build (cố định, không rải vào thư mục tạm của máy).
const targetDir = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(repo, 'target');
fs.mkdirSync(targetDir, { recursive: true });
const raw = path.join(targetDir, 'licenses-raw.html');
let html;
try {
  run(['about', 'generate', '--frozen', '--workspace', '--config', path.join(dir, 'about.toml'), '--output-file', raw,
    path.join(dir, 'template.hbs')]);
  html = readText(raw);
} finally {
  fs.rmSync(raw, { force: true });
}

const articles = extra.map((c) => [
  `<article data-license="${escapeHtml(c.license)}" data-component="${escapeHtml(c.name)}">`,
  `<h3>${escapeHtml(c.name)} ${escapeHtml(c.version)}: ${escapeHtml(c.license)}</h3>`,
  `<p class="note"><a href="${escapeHtml(c.url)}">${escapeHtml(c.url)}</a></p>`,
  `<pre>${escapeHtml(readText(path.resolve(dir, c.text)))}</pre>`,
  '</article>',
].join('\n'));
const marker = '<!-- extra-components -->';
if (!html.includes(marker)) fail(`template has no ${marker}`);
html = html.replace(marker, () => articles.join('\n'));

// D-2: soát nhãn (không soát cả văn bản: MPL có nhắc tên GPL mà không áp dụng). Riêng crate thì nhãn LGPL nào cũng là lỗi.
const gpl = /\b(a|l)?gpl/i;
const crateLabels = unique([...html.matchAll(/<article data-license="([^"]+)">/g)].map((m) => m[1]));
const banned = crateLabels.filter((label) => gpl.test(label));
if (banned.length) fail(`banned license: ${banned.join(', ')}`);
const bannedComponents = [...html.matchAll(/data-license="([^"]+)" data-component="([^"]+)"/g)]
  .map((m) => ({ license: m[1], name: decodeHtml(m[2]) }))
  .filter((c) => gpl.test(c.license) && c.name !== lgplAllowed);
if (bannedComponents.length) fail(`banned license: ${bannedComponents.map((c) => c.name).join(', ')}`);

const crates = unique([...html.matchAll(/data-crate="([^"]+)"/g)].map((m) => m[1]));
const absent = required.filter((name) => !crates.includes(name));
if (absent.length) fail(`missing from the page: ${absent.join(', ')}`);

fs.mkdirSync(out, { recursive: true });
const file = path.join(path.resolve(out), 'third-party.html');
fs.writeFileSync(file, html);
console.log(`crates=${crates.length} licenses=${crateLabels.join(', ')}`);
console.log(`sha256=${crypto.createHash('sha256').update(html).digest('hex')}`);
console.log(`wrote ${file}`);
