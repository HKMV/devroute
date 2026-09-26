// 一次性脚本：给 app-icon-64.png 右下角加绿点徽标 → app-icon-running-64.png
const zlib = require('zlib'), fs = require('fs');

function crc32(buf) {
  let c, table = [];
  for (let n = 0; n < 256; n++) { c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; table[n] = c; }
  let crc = 0xffffffff;
  for (const b of buf) crc = table[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function decodePng(buf) {
  let pos = 8, w, h, idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos), type = buf.toString('ascii', pos + 4, pos + 8);
    const data = buf.slice(pos + 8, pos + 8 + len);
    if (type === 'IHDR') { w = data.readUInt32BE(0); h = data.readUInt32BE(4); if (data[8] !== 8 || data[9] !== 6) throw new Error('need 8-bit RGBA'); }
    if (type === 'IDAT') idat.push(data);
    pos += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * 4, px = Buffer.alloc(w * h * 4);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)], line = raw.slice(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const cur = px.slice(y * stride, (y + 1) * stride);
    for (let i = 0; i < stride; i++) {
      const a = i >= 4 ? cur[i - 4] : 0, b = prev[i], c = i >= 4 ? prev[i - 4] : 0;
      let v;
      if (f === 0) v = line[i];
      else if (f === 1) v = line[i] + a;
      else if (f === 2) v = line[i] + b;
      else if (f === 3) v = line[i] + ((a + b) >> 1);
      else { const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c); v = line[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c); }
      cur[i] = v & 0xff;
    }
    prev = cur;
  }
  return { w, h, px };
}

function encodePng(w, h, px) {
  const stride = w * 4, raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) { raw[y * (stride + 1)] = 0; px.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride); }
  const chunk = (type, data) => { const b = Buffer.alloc(12 + data.length); b.writeUInt32BE(data.length, 0); b.write(type, 4, 'ascii'); data.copy(b, 8); b.writeUInt32BE(crc32(b.slice(4, 8 + data.length)), 8 + data.length); return b; };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

const { w, h, px } = decodePng(fs.readFileSync('assets/app-icon-64.png'));
// 运行态：蓝色元素（尖括号/云）整体换成绿色，新芽本来就是绿的；白底和明度不变
for (let i = 0; i < w * h; i++) {
  const r = px[i * 4], g = px[i * 4 + 1], b = px[i * 4 + 2];
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  const v = max / 255, s = max === 0 ? 0 : (max - min) / max;
  let hue = 0;
  if (max !== min) {
    const d = max - min;
    if (max === r) hue = ((g - b) / d + 6) % 6;
    else if (max === g) hue = (b - r) / d + 2;
    else hue = (r - g) / d + 4;
    hue *= 60;
  }
  // 蓝色区间（含青蓝）→ 橙色（运行态）；纯白/纯灰饱和度为 0 不受影响
  if (s > 0.15 && hue >= 170 && hue <= 260) {
    const nh = 38 / 60, nv = v, ns = s;
    const c = nv * ns, x = c * (1 - Math.abs((nh % 2) - 1)), m = nv - c;
    let nr, ng, nb;
    if (nh < 1) [nr, ng, nb] = [c, x, 0];
    else if (nh < 2) [nr, ng, nb] = [x, c, 0];
    else if (nh < 3) [nr, ng, nb] = [0, c, x];
    else if (nh < 4) [nr, ng, nb] = [0, x, c];
    else if (nh < 5) [nr, ng, nb] = [x, 0, c];
    else [nr, ng, nb] = [c, 0, x];
    px[i * 4] = Math.round((nr + m) * 255);
    px[i * 4 + 1] = Math.round((ng + m) * 255);
    px[i * 4 + 2] = Math.round((nb + m) * 255);
  }
}
fs.writeFileSync('assets/app-icon-running-64.png', encodePng(w, h, px));
console.log('ok', w, h);
