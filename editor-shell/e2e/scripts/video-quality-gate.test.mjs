import assert from "node:assert/strict";
import test from "node:test";

import {
  QUALITY_THRESHOLDS,
  evaluateProbe,
  parseRational,
  representativeTimestamps,
} from "./video-quality-gate.mjs";

function fixture({
  duration = "180.000000",
  fps = "30/1",
  frames = "5400",
  codec = "h264",
  width = 1920,
  height = 1080,
  audio = false,
} = {}) {
  return {
    streams: [
      {
        index: 0,
        codec_type: "video",
        codec_name: codec,
        width,
        height,
        duration,
        avg_frame_rate: fps,
        r_frame_rate: fps,
        nb_read_frames: frames,
        disposition: { attached_pic: 0 },
      },
      ...(audio
        ? [{ index: 1, codec_type: "audio", codec_name: "aac", sample_rate: "48000", channels: 2 }]
        : []),
    ],
    format: { duration, format_name: "mov,mp4,m4a,3gp,3g2,mj2" },
  };
}

test("parses exact and fractional frame rates without accepting malformed values", () => {
  assert.equal(parseRational("30/1"), 30);
  assert.equal(parseRational("60000/2000"), 30);
  assert.equal(parseRational(60), 60);
  assert.equal(parseRational("0/0"), null);
  assert.equal(parseRational("N/A"), null);
  assert.equal(parseRational("30/1/2"), null);
});

test("accepts a 180-second, 30 fps, 1080p H.264 clip without audio", () => {
  const result = evaluateProbe(fixture());
  assert.equal(result.passed, true);
  assert.equal(result.normalized.audioStreamCount, 0);
  assert.deepEqual(result.checks.filter((item) => !item.passed), []);
});

test("accepts H.265 and AV1 and records optional audio without requiring it", () => {
  for (const codec of ["hevc", "av1"]) {
    const result = evaluateProbe(fixture({ codec, audio: true }));
    assert.equal(result.passed, true, codec);
    assert.equal(result.normalized.audioStreamCount, 1);
  }
});

test("fails every hard delivery threshold independently", () => {
  const cases = [
    ["duration", { duration: "179.999", frames: "5400" }],
    ["frame-rate", { fps: "30000/1001", frames: "5395" }],
    ["codec", { codec: "vp9" }],
    ["resolution", { width: 1279 }],
    ["resolution", { height: 719 }],
    ["frame-count-present", { frames: "N/A" }],
    ["frame-count-consistency", { frames: "5000" }],
  ];
  for (const [expectedFailure, overrides] of cases) {
    const result = evaluateProbe(fixture(overrides));
    assert.equal(result.passed, false, expectedFailure);
    assert.equal(result.checks.find((item) => item.id === expectedFailure)?.passed, false, expectedFailure);
  }
});

test("uses the counted frame total and applies the documented bounded consistency tolerance", () => {
  const withinTolerance = evaluateProbe(fixture({ frames: "5370" }));
  const outsideTolerance = evaluateProbe(fixture({ frames: "5369" }));
  assert.equal(withinTolerance.checks.find((item) => item.id === "frame-count-consistency").passed, true);
  assert.equal(outsideTolerance.checks.find((item) => item.id === "frame-count-consistency").passed, false);
  assert.equal(withinTolerance.normalized.frameCountSource, "nb_read_frames");
  assert.equal(QUALITY_THRESHOLDS.maximumFrameDurationMismatchSeconds, 1);
});

test("chooses deterministic midpoint samples across the complete clip", () => {
  assert.deepEqual(representativeTimestamps(180, 4), [22.5, 67.5, 112.5, 157.5]);
  const timestamps = representativeTimestamps(187.5, 12);
  assert.equal(timestamps.length, 12);
  assert.equal(timestamps[0], 7.8125);
  assert.equal(timestamps.at(-1), 179.6875);
  assert.throws(() => representativeTimestamps(0, 12), /positive finite duration/);
});

