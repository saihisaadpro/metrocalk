// Generate real Radiance `.hdr` fixtures for the environment-import tests.
//
// These are WRITTEN rather than checked in as binaries so the corpus stays reviewable in a diff and
// carries no licensing question — and, more importantly, so the expected radiance is stated in code
// next to the test that asserts it. A fixture whose contents nobody can read is a fixture nobody can
// debug when it fails.
//
// Radiance RGBE: a text header, a resolution line, then 4 bytes per pixel. A shared exponent E encodes
// the mantissas R,G,B as `value = (M + 0.5) / 256 * 2^(E - 128)`, which is how one byte per channel
// still carries high dynamic range.

import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
mkdirSync(here, { recursive: true });

/** Encode one linear RGB triple as a 4-byte RGBE pixel. */
function rgbe(r, g, b) {
  const peak = Math.max(r, g, b);
  if (peak < 1e-32) return [0, 0, 0, 0];
  // frexp: peak = mantissa * 2^exp, with mantissa in [0.5, 1)
  let exp = Math.ceil(Math.log2(peak));
  const scale = 256 / Math.pow(2, exp);
  return [
    Math.min(255, Math.floor(r * scale)),
    Math.min(255, Math.floor(g * scale)),
    Math.min(255, Math.floor(b * scale)),
    exp + 128,
  ];
}

/**
 * Write an equirectangular Radiance panorama.
 * `shade(u, v)` returns linear `[r, g, b]` for normalised coordinates.
 */
function writeHdr(file, width, height, shade) {
  const header = Buffer.from(
    `#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nSOFTWARE=metrocalk-fixture\n\n-Y ${height} +X ${width}\n`,
    "ascii",
  );
  const pixels = Buffer.alloc(width * height * 4);
  let o = 0;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const [r, g, b, e] = rgbe(...shade(x / width, y / height));
      pixels[o++] = r;
      pixels[o++] = g;
      pixels[o++] = b;
      pixels[o++] = e;
    }
  }
  writeFileSync(file, Buffer.concat([header, pixels]));
  return file;
}

// ── A UNIFORM STRONG RED sky, radiance 8.0 ────────────────────────────────────────────────────
// Deliberately not subtle. A test that asserts "the render changed a bit" is a test that passes on
// noise; a red sky either reaches the viewport or it does not, and a pixel probe can say which.
const red = path.join(here, "env-red.hdr");
writeHdr(red, 64, 32, () => [8.0, 0.05, 0.05]);

// ── A UNIFORM STRONG BLUE sky ─────────────────────────────────────────────────────────────────
// The second colour proves the FIRST result was the environment and not some other reddish thing:
// importing this must move the viewport from red to blue.
const blue = path.join(here, "env-blue.hdr");
writeHdr(blue, 64, 32, () => [0.05, 0.1, 8.0]);

// ── A NON-EQUIRECTANGULAR image ───────────────────────────────────────────────────────────────
// Square, so its aspect is 1:1 rather than the 2:1 an equirect panorama requires. Must be REFUSED
// with a reason, not stretched onto the sphere.
const square = path.join(here, "env-square.hdr");
writeHdr(square, 32, 32, () => [1.0, 1.0, 1.0]);

// ── A CORRUPT file ────────────────────────────────────────────────────────────────────────────
// A valid Radiance header followed by truncated scanline data: the decoder must refuse it rather
// than reading past the end or hanging.
const corrupt = path.join(here, "env-corrupt.hdr");
writeFileSync(
  corrupt,
  Buffer.concat([
    Buffer.from("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 32 +X 64\n", "ascii"),
    Buffer.alloc(37), // nowhere near 64*32*4
  ]),
);

console.log(`wrote ${red}\nwrote ${blue}\nwrote ${square}\nwrote ${corrupt}`);
