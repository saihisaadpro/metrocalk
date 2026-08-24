#!/usr/bin/env node

// Deterministic, local validation for a finished Metrocalk cinematic. This utility deliberately
// invokes explicit ffmpeg/ffprobe executables without a shell, writes reviewable frame evidence,
// and fails closed when any required signal is missing.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const THIS_FILE = fileURLToPath(import.meta.url);

export const QUALITY_THRESHOLDS = Object.freeze({
  minimumDurationSeconds: 180,
  minimumFramesPerSecond: 30,
  minimumWidth: 1280,
  minimumHeight: 720,
  allowedVideoCodecs: Object.freeze(["h264", "hevc", "av1"]),
  maximumFrameCountRelativeError: 0.01,
  maximumFrameDurationMismatchSeconds: 1,
});

const DEFAULT_SAMPLE_COUNT = 12;
const SAMPLE_COLUMNS = 4;
const THUMBNAIL_WIDTH = 480;
const THUMBNAIL_HEIGHT = 270;
const THUMBNAIL_GAP = 8;
const MANIFEST_NAME = "video-quality-validation.json";
const CONTACT_SHEET_NAME = "video-quality-contact-sheet.png";
const SAMPLE_DIRECTORY_NAME = "video-quality-samples";
const NULL_DEVICE = process.platform === "win32" ? "NUL" : "/dev/null";

const usage = `Usage:
  node scripts/video-quality-gate.mjs \\
    --input <absolute-video-path> \\
    --output-dir <absolute-evidence-directory> \\
    --ffmpeg <absolute-ffmpeg-executable-path> \\
    --ffprobe <absolute-ffprobe-executable-path> \\
    [--sample-count <4-24>]

The output directory may contain other evidence, but this command refuses to overwrite an existing
video-quality-validation.json, video-quality-contact-sheet.png, or video-quality-samples directory.`;

function finiteNumber(value) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function finiteInteger(value) {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : null;
}

export function parseRational(value) {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value !== "string" || value.length === 0 || value === "N/A") return null;
  const parts = value.split("/");
  if (parts.length === 1) return finiteNumber(parts[0]);
  if (parts.length !== 2) return null;
  const numerator = finiteNumber(parts[0]);
  const denominator = finiteNumber(parts[1]);
  if (numerator === null || denominator === null || denominator === 0) return null;
  const result = numerator / denominator;
  return Number.isFinite(result) ? result : null;
}

function check(id, passed, expected, actual, detail) {
  return { id, passed: Boolean(passed), expected, actual, detail };
}

export function evaluateProbe(probe, thresholds = QUALITY_THRESHOLDS) {
  const streams = Array.isArray(probe?.streams) ? probe.streams : [];
  const videoStreams = streams.filter(
    (stream) => stream?.codec_type === "video" && Number(stream?.disposition?.attached_pic ?? 0) !== 1,
  );
  const audioStreams = streams.filter((stream) => stream?.codec_type === "audio");
  const video = videoStreams[0] ?? null;

  const durationSeconds =
    finiteNumber(video?.duration) ?? finiteNumber(probe?.format?.duration) ?? null;
  const framesPerSecond =
    parseRational(video?.avg_frame_rate) ?? parseRational(video?.r_frame_rate) ?? null;
  const countedFrames = finiteInteger(video?.nb_read_frames);
  const declaredFrames = finiteInteger(video?.nb_frames);
  const frameCount = countedFrames ?? declaredFrames;
  const frameCountSource = countedFrames !== null ? "nb_read_frames" : declaredFrames !== null ? "nb_frames" : null;
  const width = finiteInteger(video?.width);
  const height = finiteInteger(video?.height);
  const codec = typeof video?.codec_name === "string" ? video.codec_name.toLowerCase() : null;

  const expectedFrameCount =
    durationSeconds !== null && framesPerSecond !== null ? durationSeconds * framesPerSecond : null;
  const frameCountDifference =
    expectedFrameCount !== null && frameCount !== null ? Math.abs(frameCount - expectedFrameCount) : null;
  const frameCountRelativeError =
    frameCountDifference !== null && expectedFrameCount > 0 ? frameCountDifference / expectedFrameCount : null;
  const allowedFrameCountDifference =
    expectedFrameCount !== null && framesPerSecond !== null
      ? Math.min(
          expectedFrameCount * thresholds.maximumFrameCountRelativeError,
          framesPerSecond * thresholds.maximumFrameDurationMismatchSeconds,
        )
      : null;

  const checks = [
    check(
      "video-stream",
      video !== null,
      "one decodable primary video stream",
      videoStreams.length,
      video === null ? "No non-cover-art video stream was reported." : `Using stream index ${video.index ?? 0}.`,
    ),
    check(
      "duration",
      durationSeconds !== null && durationSeconds >= thresholds.minimumDurationSeconds,
      `>= ${thresholds.minimumDurationSeconds} seconds`,
      durationSeconds,
      durationSeconds === null
        ? "Neither the video stream nor container reported a finite duration."
        : `${durationSeconds.toFixed(6)} seconds reported.`,
    ),
    check(
      "frame-rate",
      framesPerSecond !== null && framesPerSecond >= thresholds.minimumFramesPerSecond,
      `>= ${thresholds.minimumFramesPerSecond} fps`,
      framesPerSecond,
      framesPerSecond === null ? "No finite average or nominal frame rate was reported." : `${framesPerSecond.toFixed(6)} fps reported.`,
    ),
    check(
      "frame-count-present",
      frameCount !== null && frameCount > 0,
      "positive ffprobe frame count",
      frameCount,
      frameCountSource === null
        ? "ffprobe reported neither nb_read_frames nor nb_frames."
        : `${frameCount} frames from ${frameCountSource}.`,
    ),
    check(
      "frame-count-consistency",
      frameCountDifference !== null &&
        allowedFrameCountDifference !== null &&
        frameCountDifference <= allowedFrameCountDifference,
      `difference <= min(${thresholds.maximumFrameCountRelativeError * 100}% of expected frames, ${thresholds.maximumFrameDurationMismatchSeconds} second of frames)`,
      frameCountDifference,
      frameCountDifference === null || allowedFrameCountDifference === null
        ? "Duration, frame rate, and frame count are all required for consistency validation."
        : `${frameCountDifference.toFixed(3)} frame difference; ${allowedFrameCountDifference.toFixed(3)} allowed.`,
    ),
    check(
      "codec",
      codec !== null && thresholds.allowedVideoCodecs.includes(codec),
      thresholds.allowedVideoCodecs,
      codec,
      codec === null ? "No video codec was reported." : `ffprobe codec_name=${codec}.`,
    ),
    check(
      "resolution",
      width !== null &&
        height !== null &&
        width >= thresholds.minimumWidth &&
        height >= thresholds.minimumHeight,
      `>= ${thresholds.minimumWidth}x${thresholds.minimumHeight}`,
      width !== null && height !== null ? `${width}x${height}` : null,
      width === null || height === null ? "No finite video dimensions were reported." : `${width}x${height} coded frame.`,
    ),
  ];

  return {
    passed: checks.every((item) => item.passed),
    checks,
    normalized: {
      streamIndex: video?.index ?? null,
      durationSeconds,
      framesPerSecond,
      frameCount,
      frameCountSource,
      expectedFrameCount,
      frameCountDifference,
      frameCountRelativeError,
      codec,
      codecLongName: video?.codec_long_name ?? null,
      profile: video?.profile ?? null,
      pixelFormat: video?.pix_fmt ?? null,
      width,
      height,
      bitRate: finiteInteger(video?.bit_rate) ?? finiteInteger(probe?.format?.bit_rate),
      audioStreamCount: audioStreams.length,
      audioStreams: audioStreams.map((stream) => ({
        index: stream.index ?? null,
        codec: stream.codec_name ?? null,
        sampleRate: finiteInteger(stream.sample_rate),
        channels: finiteInteger(stream.channels),
        channelLayout: stream.channel_layout ?? null,
      })),
      formatName: probe?.format?.format_name ?? null,
    },
  };
}

export function representativeTimestamps(durationSeconds, sampleCount = DEFAULT_SAMPLE_COUNT) {
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) {
    throw new Error("A positive finite duration is required to choose representative frames.");
  }
  if (!Number.isSafeInteger(sampleCount) || sampleCount < 1) {
    throw new Error("A positive integer sample count is required.");
  }
  return Array.from({ length: sampleCount }, (_, index) =>
    Number((((index + 0.5) * durationSeconds) / sampleCount).toFixed(6)),
  );
}

function parseArguments(argv) {
  if (argv.includes("--help") || argv.includes("-h")) return { help: true };
  const allowed = new Set(["--input", "--output-dir", "--ffmpeg", "--ffprobe", "--sample-count"]);
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!allowed.has(flag)) throw new Error(`Unknown argument: ${flag ?? "<missing>"}`);
    if (value === undefined || value.startsWith("--")) throw new Error(`Missing value for ${flag}.`);
    if (values[flag] !== undefined) throw new Error(`Duplicate argument: ${flag}`);
    values[flag] = value;
  }

  for (const required of ["--input", "--output-dir", "--ffmpeg", "--ffprobe"]) {
    if (values[required] === undefined) throw new Error(`Missing required argument: ${required}`);
    if (!path.isAbsolute(values[required])) throw new Error(`${required} must be an absolute path.`);
  }

  const sampleCount = values["--sample-count"] === undefined
    ? DEFAULT_SAMPLE_COUNT
    : Number.parseInt(values["--sample-count"], 10);
  if (!Number.isSafeInteger(sampleCount) || sampleCount < 4 || sampleCount > 24) {
    throw new Error("--sample-count must be an integer from 4 through 24.");
  }

  return {
    help: false,
    input: path.resolve(values["--input"]),
    outputDirectory: path.resolve(values["--output-dir"]),
    ffmpeg: path.resolve(values["--ffmpeg"]),
    ffprobe: path.resolve(values["--ffprobe"]),
    sampleCount,
  };
}

function assertReadableFile(file, label) {
  if (!fs.existsSync(file)) throw new Error(`${label} does not exist: ${file}`);
  const stats = fs.statSync(file);
  if (!stats.isFile() || stats.size <= 0) throw new Error(`${label} is not a non-empty file: ${file}`);
}

function runProcess(executable, arguments_, label) {
  const started = performance.now();
  const result = spawnSync(executable, arguments_, {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    shell: false,
    windowsHide: true,
  });
  return {
    label,
    executable,
    arguments: arguments_,
    exitCode: result.status,
    signal: result.signal ?? null,
    elapsedMilliseconds: Math.round(performance.now() - started),
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    spawnError: result.error ? String(result.error.message ?? result.error) : null,
  };
}

function requireSuccess(result) {
  if (result.exitCode !== 0 || result.spawnError) {
    const detail = result.stderr.trim() || result.stdout.trim() || result.spawnError || "no diagnostic output";
    throw new Error(`${result.label} failed with exit code ${result.exitCode ?? "<none>"}: ${detail}`);
  }
}

async function sha256(file) {
  return await new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    const stream = createReadStream(file);
    stream.on("error", reject);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", () => resolve(hash.digest("hex")));
  });
}

async function artifact(file, root = null) {
  const stats = fs.statSync(file);
  return {
    path: file,
    relativePath: root === null ? null : path.relative(root, file).replaceAll("\\", "/"),
    bytes: stats.size,
    sha256: await sha256(file),
  };
}

async function inspectTool(executable, label) {
  const version = runProcess(executable, ["-version"], `${label} version query`);
  requireSuccess(version);
  return {
    ...(await artifact(executable)),
    version: version.stdout.split(/\r?\n/u)[0]?.trim() || version.stderr.split(/\r?\n/u)[0]?.trim() || null,
  };
}

function commandEvidence(result, includeStdout = false) {
  return {
    executable: result.executable,
    arguments: result.arguments,
    exitCode: result.exitCode,
    signal: result.signal,
    elapsedMilliseconds: result.elapsedMilliseconds,
    spawnError: result.spawnError,
    stderr: result.stderr.trim(),
    ...(includeStdout ? { stdout: result.stdout.trim() } : {}),
  };
}

function writeManifestAtomically(manifestPath, manifest) {
  const temporaryPath = `${manifestPath}.tmp-${process.pid}`;
  fs.writeFileSync(temporaryPath, `${JSON.stringify(manifest, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
  fs.renameSync(temporaryPath, manifestPath);
}

function prepareOutput(options) {
  assertReadableFile(options.input, "Input video");
  assertReadableFile(options.ffmpeg, "ffmpeg executable");
  assertReadableFile(options.ffprobe, "ffprobe executable");

  fs.mkdirSync(options.outputDirectory, { recursive: true });
  const manifestPath = path.join(options.outputDirectory, MANIFEST_NAME);
  const contactSheetPath = path.join(options.outputDirectory, CONTACT_SHEET_NAME);
  const sampleDirectory = path.join(options.outputDirectory, SAMPLE_DIRECTORY_NAME);
  for (const target of [manifestPath, contactSheetPath, sampleDirectory]) {
    if (fs.existsSync(target)) throw new Error(`Refusing to overwrite existing quality-gate evidence: ${target}`);
  }
  fs.mkdirSync(sampleDirectory);
  return { manifestPath, contactSheetPath, sampleDirectory };
}

function contactSheetFilter(sampleCount) {
  const columns = Math.min(SAMPLE_COLUMNS, sampleCount);
  const rows = Math.ceil(sampleCount / columns);
  const inputs = [];
  const layouts = [];
  for (let index = 0; index < sampleCount; index += 1) {
    inputs.push(
      `[${index}:v]scale=${THUMBNAIL_WIDTH}:${THUMBNAIL_HEIGHT}:force_original_aspect_ratio=decrease:flags=lanczos,` +
        `pad=${THUMBNAIL_WIDTH}:${THUMBNAIL_HEIGHT}:(ow-iw)/2:(oh-ih)/2:color=0x101010[s${index}]`,
    );
    const column = index % columns;
    const row = Math.floor(index / columns);
    const x = THUMBNAIL_GAP + column * (THUMBNAIL_WIDTH + THUMBNAIL_GAP);
    const y = THUMBNAIL_GAP + row * (THUMBNAIL_HEIGHT + THUMBNAIL_GAP);
    layouts.push(`${x}_${y}`);
  }
  const inputLabels = Array.from({ length: sampleCount }, (_, index) => `[s${index}]`).join("");
  const width = THUMBNAIL_GAP + columns * (THUMBNAIL_WIDTH + THUMBNAIL_GAP);
  const height = THUMBNAIL_GAP + rows * (THUMBNAIL_HEIGHT + THUMBNAIL_GAP);
  inputs.push(
    `${inputLabels}xstack=inputs=${sampleCount}:layout=${layouts.join("|")}:fill=0x050505,` +
      `pad=${width}:${height}:0:0:color=0x050505[sheet]`,
  );
  return { filter: inputs.join(";"), columns, rows, width, height };
}

export async function runQualityGate(options) {
  const output = prepareOutput(options);
  const manifest = {
    schema: "metrocalk.video-quality-gate/v1",
    generatedAtUtc: new Date().toISOString(),
    passed: false,
    thresholds: QUALITY_THRESHOLDS,
    input: null,
    tools: {},
    probe: null,
    checks: [],
    strictDecode: { passed: false, command: null },
    representativeFrames: { passed: false, strategy: "midpoint of each equal-duration interval", frames: [] },
    contactSheet: { passed: false, artifact: null },
    errors: [],
  };

  try {
    console.log("Hashing cinematic and validation tools...");
    [manifest.input, manifest.tools.ffmpeg, manifest.tools.ffprobe, manifest.tools.validator] = await Promise.all([
      artifact(options.input),
      inspectTool(options.ffmpeg, "ffmpeg"),
      inspectTool(options.ffprobe, "ffprobe"),
      artifact(THIS_FILE).then((script) => ({ script, nodeVersion: process.version, platform: process.platform, architecture: process.arch })),
    ]);

    console.log("Counting frames and reading stream metadata with ffprobe...");
    const probeResult = runProcess(
      options.ffprobe,
      ["-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json", options.input],
      "ffprobe media inspection",
    );
    requireSuccess(probeResult);
    let probeJson;
    try {
      probeJson = JSON.parse(probeResult.stdout);
    } catch (error) {
      throw new Error(`ffprobe returned invalid JSON: ${error.message}`);
    }
    const evaluation = evaluateProbe(probeJson);
    manifest.probe = {
      command: commandEvidence(probeResult),
      media: evaluation.normalized,
      streamCount: Array.isArray(probeJson.streams) ? probeJson.streams.length : 0,
    };
    manifest.checks = evaluation.checks;

    const videoStreamAvailable = evaluation.checks.find((item) => item.id === "video-stream")?.passed === true;
    if (!videoStreamAvailable) throw new Error("Strict decode and visual sampling require a primary video stream.");

    console.log("Strictly decoding the complete clip with corruption treated as fatal...");
    const decodeResult = runProcess(
      options.ffmpeg,
      [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-xerror",
        "-err_detect",
        "explode",
        "-i",
        options.input,
        "-map",
        "0:v:0",
        "-map",
        "0:a?",
        "-f",
        "null",
        NULL_DEVICE,
      ],
      "ffmpeg strict full decode",
    );
    manifest.strictDecode = {
      passed: decodeResult.exitCode === 0 && !decodeResult.spawnError && decodeResult.stderr.trim().length === 0,
      command: commandEvidence(decodeResult),
    };

    const duration = evaluation.normalized.durationSeconds;
    if (duration === null || duration <= 0) throw new Error("Representative sampling requires a positive duration.");
    const timestamps = representativeTimestamps(duration, options.sampleCount);
    let samplesPassed = true;
    console.log(`Extracting ${timestamps.length} representative full-resolution frames...`);
    for (let index = 0; index < timestamps.length; index += 1) {
      const timestampSeconds = timestamps[index];
      const samplePath = path.join(output.sampleDirectory, `frame-${String(index + 1).padStart(3, "0")}.png`);
      const sampleResult = runProcess(
        options.ffmpeg,
        [
          "-nostdin",
          "-hide_banner",
          "-loglevel",
          "error",
          "-ss",
          timestampSeconds.toFixed(6),
          "-i",
          options.input,
          "-map",
          "0:v:0",
          "-frames:v",
          "1",
          "-an",
          "-sn",
          "-dn",
          "-compression_level",
          "3",
          "-n",
          samplePath,
        ],
        `representative frame ${index + 1}`,
      );
      const samplePassed =
        sampleResult.exitCode === 0 &&
        !sampleResult.spawnError &&
        sampleResult.stderr.trim().length === 0 &&
        fs.existsSync(samplePath) &&
        fs.statSync(samplePath).size > 0;
      samplesPassed &&= samplePassed;
      manifest.representativeFrames.frames.push({
        index: index + 1,
        timestampSeconds,
        passed: samplePassed,
        command: commandEvidence(sampleResult),
        artifact: samplePassed ? await artifact(samplePath, options.outputDirectory) : null,
      });
    }
    manifest.representativeFrames.passed = samplesPassed;

    if (samplesPassed) {
      console.log("Composing the representative-frame contact sheet...");
      const sheet = contactSheetFilter(timestamps.length);
      const sheetArguments = ["-nostdin", "-hide_banner", "-loglevel", "error"];
      for (const frame of manifest.representativeFrames.frames) {
        sheetArguments.push("-i", frame.artifact.path);
      }
      sheetArguments.push(
        "-filter_complex",
        sheet.filter,
        "-map",
        "[sheet]",
        "-frames:v",
        "1",
        "-compression_level",
        "3",
        "-n",
        output.contactSheetPath,
      );
      const sheetResult = runProcess(options.ffmpeg, sheetArguments, "representative-frame contact sheet");
      const sheetPassed =
        sheetResult.exitCode === 0 &&
        !sheetResult.spawnError &&
        sheetResult.stderr.trim().length === 0 &&
        fs.existsSync(output.contactSheetPath) &&
        fs.statSync(output.contactSheetPath).size > 0;
      manifest.contactSheet = {
        passed: sheetPassed,
        command: commandEvidence(sheetResult),
        layout: {
          columns: sheet.columns,
          rows: sheet.rows,
          thumbnailWidth: THUMBNAIL_WIDTH,
          thumbnailHeight: THUMBNAIL_HEIGHT,
          width: sheet.width,
          height: sheet.height,
        },
        artifact: sheetPassed ? await artifact(output.contactSheetPath, options.outputDirectory) : null,
      };
    }
  } catch (error) {
    manifest.errors.push(error instanceof Error ? error.message : String(error));
  }

  manifest.passed =
    manifest.errors.length === 0 &&
    manifest.checks.length > 0 &&
    manifest.checks.every((item) => item.passed) &&
    manifest.strictDecode.passed &&
    manifest.representativeFrames.passed &&
    manifest.contactSheet.passed;
  manifest.completedAtUtc = new Date().toISOString();
  writeManifestAtomically(output.manifestPath, manifest);
  return { manifest, manifestPath: output.manifestPath };
}

async function main() {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
  } catch (error) {
    console.error(`${error.message}\n\n${usage}`);
    process.exitCode = 2;
    return;
  }
  if (options.help) {
    console.log(usage);
    return;
  }

  try {
    const result = await runQualityGate(options);
    if (result.manifest.passed) {
      console.log(`Video quality gate passed: ${result.manifestPath}`);
    } else {
      const failedChecks = result.manifest.checks.filter((item) => !item.passed).map((item) => item.id);
      console.error(
        `Video quality gate failed (${[...failedChecks, ...result.manifest.errors].join(", ") || "artifact validation"}): ${result.manifestPath}`,
      );
      process.exitCode = 1;
    }
  } catch (error) {
    console.error(`Video quality gate could not start: ${error.message}`);
    process.exitCode = 2;
  }
}

const invokedAsScript = process.argv[1] && path.resolve(process.argv[1]) === path.resolve(THIS_FILE);
if (invokedAsScript) await main();
