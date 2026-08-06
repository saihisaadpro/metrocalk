// M15.11 — the thumbnail RTT goes through the SAME final resolve as the swapchain frame.
//
// It is the one other place scene geometry is rasterised, and the migration changed it completely: it now
// renders into a linear-HDR target at the scene sample count, resolves MSAA in linear light, and runs
// `fs_resolve` into a display-format readback texture. That path has no window, so a viewport capture
// cannot see it — and a broken resolve there is exactly the kind of thing that ships silently as
// "thumbnails are a bit dark".
//
// The check is on the returned PNG itself: decoded in the page, it must not be uniformly black (a missing
// resolve), must not be uniformly white (a double encode blowing out), and must carry real variation.

import { invoke } from "../pages/scaffold.js";

/** Decode a data: PNG in the page and report simple statistics over its pixels. */
async function pngStats(dataUrl) {
  return browser.executeAsync((url, done) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement("canvas");
      c.width = img.width;
      c.height = img.height;
      const ctx = c.getContext("2d");
      ctx.drawImage(img, 0, 0);
      const { data } = ctx.getImageData(0, 0, c.width, c.height);
      let min = 255;
      let max = 0;
      let sum = 0;
      let n = 0;
      const seen = new Set();
      for (let i = 0; i < data.length; i += 4) {
        const l = 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
        min = Math.min(min, l);
        max = Math.max(max, l);
        sum += l;
        n += 1;
        seen.add(`${data[i] >> 3},${data[i + 1] >> 3},${data[i + 2] >> 3}`);
      }
      done({ w: img.width, h: img.height, min, max, mean: sum / n, distinct: seen.size });
    };
    img.onerror = () => done(null);
    img.src = url;
  }, dataUrl);
}

describe("HDR pipeline — the thumbnail RTT", () => {
  it("renders an entity through the same final resolve and returns a graded PNG", async () => {
    // Any entity with a renderable instance will do; take the first the scene reports.
    const id = await invoke("viewport_pick", { x: 0.5, y: 0.5 });
    expect(id).toBeTruthy();

    let url = null;
    await browser.waitUntil(
      async () => {
        url = await invoke("thumbnail", { id, size: 128 });
        return typeof url === "string" && url.startsWith("data:image/png;base64,");
      },
      { timeout: 30000, interval: 500, timeoutMsg: "the thumbnail RTT never produced a PNG" },
    );

    const stats = await pngStats(url);
    // eslint-disable-next-line no-console
    console.log(`[hdr-thumbnail] ${JSON.stringify(stats)}`);
    expect(stats).toBeTruthy();
    // Square, and inside the size range `render_thumbnail` clamps to. Not an equality check on the
    // requested size: the command is free to clamp, and what this spec is actually about is the grade.
    expect(stats.w).toBe(stats.h);
    expect(stats.w).toBeGreaterThanOrEqual(32);
    expect(stats.w).toBeLessThanOrEqual(256);
    // Not uniformly black: a missing or mis-bound final resolve leaves the readback texture cleared.
    expect(stats.max).toBeGreaterThan(40);
    // Not uniformly white: a DOUBLE display encode (shader OETF over an sRGB target) blows the whole
    // image out, which is the failure this pipeline is most at risk of.
    expect(stats.min).toBeLessThan(230);
    // Real content, not a flat fill — the entity, its background and its shading are all in there.
    expect(stats.distinct).toBeGreaterThan(8);
    // The background is the authored clear colour, which is dark; the subject is lit. So the mean has to
    // sit in the middle of the range rather than pinned at either end.
    expect(stats.mean).toBeGreaterThan(10);
    expect(stats.mean).toBeLessThan(230);
  });
});
