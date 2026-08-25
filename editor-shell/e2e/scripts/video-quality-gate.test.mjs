import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  QUALITY_THRESHOLDS,
  analyzeVisualContent,
  evaluateProbe,
  evaluateVisualContent,
  parseRational,
  parseSignalStatsMetadata,
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

test("parses FFmpeg signalstats metadata into timestamped numeric evidence", () => {
  const frames = parseSignalStatsMetadata([
    "frame:0    pts:0       pts_time:0",
    "lavfi.signalstats.YAVG=16",
    "lavfi.signalstats.YMAX=16",
    "frame:1    pts:15      pts_time:0.5",
    "lavfi.signalstats.YAVG=84.25",
  ].join("\n"));
  assert.deepEqual(frames, [
    { frame: 0, ptsTimeSeconds: 0, metrics: { YAVG: 16, YMAX: 16 } },
    { frame: 1, ptsTimeSeconds: 0.5, metrics: { YAVG: 84.25 } },
  ]);
});

test("rejects black, near-black, and essentially static sample series while allowing cinematic holds", () => {
  const sampleCount = 90;
  const black = evaluateVisualContent({
    luminanceSamples: Array(sampleCount).fill(16),
    motionSamples: Array(sampleCount - 1).fill(0),
  });
  assert.equal(black.passed, false);
  assert.equal(black.checks.find((item) => item.id === "visual-not-near-black").passed, false);
  assert.equal(black.checks.find((item) => item.id === "visual-not-static").passed, false);

  const nearBlack = evaluateVisualContent({
    luminanceSamples: Array(sampleCount).fill(19),
    motionSamples: Array(sampleCount - 1).fill(3),
  });
  assert.equal(nearBlack.checks.find((item) => item.id === "visual-not-near-black").passed, false);

  const staticBright = evaluateVisualContent({
    luminanceSamples: Array(sampleCount).fill(96),
    motionSamples: Array(sampleCount - 1).fill(0),
  });
  assert.equal(staticBright.checks.find((item) => item.id === "visual-not-near-black").passed, true);
  assert.equal(staticBright.checks.find((item) => item.id === "visual-not-static").passed, false);

  const mostlyHeldCinematic = evaluateVisualContent({
    luminanceSamples: Array.from({ length: sampleCount }, (_, index) => 48 + (index % 5)),
    motionSamples: Array.from({ length: sampleCount - 1 }, (_, index) => (index < 10 ? 2.5 : 0)),
    contrastSamples: Array(sampleCount).fill(95),
  });
  assert.equal(mostlyHeldCinematic.passed, true);
  assert.equal(mostlyHeldCinematic.motion.movingTransitionFraction > 0.08, true);
});

test("rejects a film whose frames are filled by one surface, and names how many", () => {
  // The fifteen interdecile-luma readings measured on the first delivered factory film. Five of them
  // are the camera inside or hard against geometry; the film passed every other check in this gate.
  const deliveredFilm = [103, 26, 29, 5, 124, 132, 137, 105, 56, 62, 12, 123, 39, 90, 91];
  const brightAndMoving = {
    luminanceSamples: Array(deliveredFilm.length).fill(96),
    motionSamples: Array(deliveredFilm.length - 1).fill(2.5),
  };
  const obscured = evaluateVisualContent({
    ...brightAndMoving,
    contrastSamples: deliveredFilm,
  });
  const legibility = obscured.checks.find((item) => item.id === "visual-frames-legible");
  assert.equal(legibility.passed, false);
  assert.equal(legibility.actual.legibleFrames, 10);
  assert.equal(legibility.actual.sampledFrames, 15);
  // Every other visual check still passes: a wall two centimetres from the lens is a well-exposed,
  // perfectly stable picture, which is exactly why this check had to be added.
  assert.equal(obscured.checks.find((item) => item.id === "visual-not-near-black").passed, true);
  assert.equal(obscured.checks.find((item) => item.id === "visual-not-static").passed, true);

  // The same film with its five obscured frames re-aimed passes.
  const rescued = evaluateVisualContent({
    ...brightAndMoving,
    contrastSamples: deliveredFilm.map((value) => (value < 40 ? 88 : value)),
  });
  assert.equal(rescued.checks.find((item) => item.id === "visual-frames-legible").passed, true);
  assert.equal(rescued.passed, true);

  // Missing data is never a pass. A caller that does not measure contrast has not shown the film is
  // legible, and this check reports that rather than waving it through.
  const unmeasured = evaluateVisualContent(brightAndMoving);
  assert.equal(unmeasured.checks.find((item) => item.id === "visual-frames-legible").passed, false);
});

function discoverFfmpeg() {
  const candidates = [
    process.env.MTK_FFMPEG_PATH,
    process.env.FFMPEG_PATH,
    "C:\\Users\\saihi\\AppData\\Local\\Microsoft\\WinGet\\Packages\\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe\\ffmpeg-8.0.1-full_build\\bin\\ffmpeg.exe",
  ].filter(Boolean);
  if (process.platform === "win32") {
    const where = spawnSync("where.exe", ["ffmpeg"], { encoding: "utf8", windowsHide: true });
    if (where.status === 0) candidates.push(...where.stdout.split(/\r?\n/u).filter(Boolean));
  } else {
    const which = spawnSync("which", ["ffmpeg"], { encoding: "utf8" });
    if (which.status === 0) candidates.push(...which.stdout.split(/\r?\n/u).filter(Boolean));
  }
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? null;
}

function makeSyntheticVideo(ffmpeg, output, source) {
  const result = spawnSync(
    ffmpeg,
    [
      "-nostdin",
      "-hide_banner",
      "-loglevel",
      "error",
      "-f",
      "lavfi",
      "-i",
      source,
      "-c:v",
      "mpeg4",
      "-q:v",
      "2",
      "-pix_fmt",
      "yuv420p",
      "-y",
      output,
    ],
    { encoding: "utf8", windowsHide: true },
  );
  assert.equal(result.status, 0, result.stderr || `Could not create ${output}`);
}

test("FFmpeg evidence rejects black/static footage and accepts moving footage", { timeout: 30_000 }, (context) => {
  const ffmpeg = discoverFfmpeg();
  if (!ffmpeg) {
    context.skip("ffmpeg is unavailable in this environment");
    return;
  }

  const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "mtk-video-content-gate-"));
  try {
    const blackPath = path.join(temporaryDirectory, "black.mp4");
    const staticPath = path.join(temporaryDirectory, "static.mp4");
    const movingPath = path.join(temporaryDirectory, "moving.mp4");
    makeSyntheticVideo(ffmpeg, blackPath, "color=c=black:size=320x180:rate=12:duration=4");
    makeSyntheticVideo(ffmpeg, staticPath, "color=c=blue:size=320x180:rate=12:duration=4");
    makeSyntheticVideo(ffmpeg, movingPath, "testsrc2=size=320x180:rate=12:duration=4");

    const analysisOptions = { ffmpeg, intervalSeconds: 0.25 };
    const black = analyzeVisualContent({ ...analysisOptions, input: blackPath });
    const staticImage = analyzeVisualContent({ ...analysisOptions, input: staticPath });
    const moving = analyzeVisualContent({ ...analysisOptions, input: movingPath });

    assert.equal(black.passed, false);
    assert.equal(black.checks.find((item) => item.id === "visual-not-near-black").passed, false);
    assert.equal(staticImage.passed, false);
    assert.equal(staticImage.checks.find((item) => item.id === "visual-not-static").passed, false);
    assert.equal(moving.passed, true);
    assert.equal(moving.luminance.samples.length >= QUALITY_THRESHOLDS.minimumVisualSamples, true);
    assert.equal(moving.motion.samples.length >= QUALITY_THRESHOLDS.minimumVisualSamples - 1, true);
    assert.match(moving.luminance.command.arguments.join(" "), /signalstats/u);
    assert.match(moving.motion.command.arguments.join(" "), /tblend=all_mode=difference/u);
  } finally {
    fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  }
});
