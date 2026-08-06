import init, { SstvReceiver } from './pkg/rssstv_web_demo.js';

const FILE_CHUNK_SAMPLES = 16384;
const FILE_YIELD_EVERY = 8;
const FILE_STAGING_MARGIN_SECONDS = 30;
const MIC_STAGING_SECONDS = 360;
const MIC_TAIL_SECONDS = 15;
const MIC_STALL_SECONDS = 20;
const MIC_BLOCK_SAMPLES = 2048;

const ui = {
  file: document.getElementById('file'),
  micStart: document.getElementById('mic-start'),
  micStop: document.getElementById('mic-stop'),
  autoReset: document.getElementById('auto-reset'),
  reset: document.getElementById('reset'),
  download: document.getElementById('download'),
  canvas: document.getElementById('canvas'),
  error: document.getElementById('error'),
  mode: document.getElementById('mode'),
  state: document.getElementById('state'),
  rows: document.getElementById('rows'),
  level: document.getElementById('level'),
  offset: document.getElementById('offset'),
  effective: document.getElementById('effective'),
  callsigns: document.getElementById('callsigns'),
  log: document.getElementById('log'),
};

const context2d = ui.canvas.getContext('2d');

const session = {
  receiver: null,
  painted: -1,
  decoding: false,
  mic: null,
  completedAt: null,
  rowsAt: 0,
  rows: 0,
};

await init();

/** Reads a status snapshot and releases the object wasm-bindgen handed over. */
function readStatus() {
  const status = session.receiver.status();
  try {
    return {
      modeName: status.mode_name,
      state: status.state,
      completedRows: status.completed_rows,
      width: status.width,
      height: status.height,
      imageRevision: status.image_revision,
      frequencyOffsetHz: status.frequency_offset_hz,
      effectiveSampleRateHz: status.effective_sample_rate_hz,
      callsigns: status.callsigns,
      level: status.level,
      finished: status.finished,
    };
  } finally {
    status.free();
  }
}

function replaceReceiver(sampleRateHz, liveSlant, stagingSeconds) {
  // Built before the old one is released, so a rejected sample rate leaves the
  // previous reception in place rather than a freed handle behind it.
  const receiver = new SstvReceiver(sampleRateHz, liveSlant, stagingSeconds);
  session.receiver?.free();
  session.receiver = receiver;
  session.painted = -1;
  session.completedAt = null;
  session.rows = 0;
  session.rowsAt = performance.now();
  ui.log.replaceChildren();
  ui.reset.disabled = false;
  ui.download.disabled = false;
  context2d.clearRect(0, 0, ui.canvas.width, ui.canvas.height);
}

function showError(error) {
  ui.error.textContent = String(error?.message ?? error);
  ui.error.hidden = false;
}

function clearError() {
  ui.error.hidden = true;
}

function appendLog(entries) {
  for (const entry of entries) {
    const item = document.createElement('li');
    item.textContent = entry;
    ui.log.append(item);
  }
  if (entries.length > 0) {
    ui.log.scrollTop = ui.log.scrollHeight;
  }
}

function paint(status) {
  if (status.width === 0 || status.height === 0) {
    return;
  }
  if (status.imageRevision === session.painted) {
    return;
  }
  const rgba = session.receiver.image_rgba();
  if (rgba === undefined) {
    return;
  }
  if (ui.canvas.width !== status.width || ui.canvas.height !== status.height) {
    ui.canvas.width = status.width;
    ui.canvas.height = status.height;
  }
  const pixels = new Uint8ClampedArray(rgba.buffer, rgba.byteOffset, rgba.length);
  context2d.putImageData(new ImageData(pixels, status.width, status.height), 0, 0);
  session.painted = status.imageRevision;
}

function render(status) {
  ui.mode.textContent = status.modeName ?? '—';
  ui.state.textContent = status.state;
  ui.rows.textContent = status.height > 0 ? `${status.completedRows} / ${status.height}` : '—';
  ui.level.value = status.level;
  ui.offset.textContent = status.modeName ? `${status.frequencyOffsetHz.toFixed(2)} Hz` : '—';
  ui.effective.textContent =
    status.effectiveSampleRateHz === undefined
      ? '—'
      : `${status.effectiveSampleRateHz.toFixed(2)} Hz`;
  ui.callsigns.textContent = status.callsigns.length > 0 ? status.callsigns.join(', ') : '—';
}

/** Ends the reception, which is also what applies the whole-stream slant refit. */
function finishReception() {
  const status = session.receiver.finish();
  try {
    appendLog([`finished: ${status.state}`]);
  } finally {
    status.free();
  }
  const snapshot = readStatus();
  paint(snapshot);
  render(snapshot);
  session.completedAt = null;
  return snapshot;
}

/**
 * The pipeline never stops on its own, and every sample after the last row is
 * staged for the refit, so a microphone left running would eventually exhaust
 * the staging bound. Ending the reception a little after the picture completes
 * is both what bounds that and what produces the refined image.
 */
function driveMicrophone(status) {
  if (session.mic === null || status.finished) {
    return;
  }
  const now = performance.now();
  if (status.state === 'complete') {
    session.completedAt ??= now;
    if (now - session.completedAt > MIC_TAIL_SECONDS * 1000) {
      finishReception();
      if (ui.autoReset.checked) {
        session.receiver.reset();
        session.painted = -1;
        session.rows = 0;
        session.rowsAt = performance.now();
      }
    }
    return;
  }
  if (status.state === 'decoding') {
    if (status.completedRows !== session.rows) {
      session.rows = status.completedRows;
      session.rowsAt = now;
    } else if (now - session.rowsAt > MIC_STALL_SECONDS * 1000) {
      appendLog(['no row for 20 s; ending the reception']);
      finishReception();
      if (ui.autoReset.checked) {
        session.receiver.reset();
        session.painted = -1;
      }
    }
  }
}

function tick() {
  requestAnimationFrame(tick);
  if (session.receiver === null) {
    return;
  }
  try {
    appendLog(session.receiver.drain_log());
    const status = readStatus();
    paint(status);
    render(status);
    driveMicrophone(status);
  } catch (error) {
    session.receiver = null;
    stopMicrophone();
    showError(error);
  }
}

requestAnimationFrame(tick);

/**
 * Reads first-channel samples straight out of a PCM WAV file.
 *
 * `decodeAudioData` is not used for these. It resamples to the context's rate,
 * and even at the file's own rate it does not return the file's samples: about
 * half of them come back off by a fraction of a least significant bit. Reading
 * the chunks here is what makes the picture identical to the one `decode-wav`
 * produces from the same file, and it accepts rates the Web Audio API will not
 * open a context at.
 */
function readWav(bytes) {
  const view = new DataView(bytes);
  if (view.byteLength < 44 || view.getUint32(0, false) !== 0x52494646) {
    return null;
  }
  if (view.getUint32(8, false) !== 0x57415645) {
    return null;
  }
  let offset = 12;
  let format = null;
  let data = null;
  while (offset + 8 <= view.byteLength) {
    const id = view.getUint32(offset, false);
    const size = view.getUint32(offset + 4, true);
    const body = offset + 8;
    if (id === 0x666d7420 && body + 16 <= view.byteLength) {
      let tag = view.getUint16(body, true);
      if (tag === 0xfffe && body + 26 <= view.byteLength) {
        tag = view.getUint16(body + 24, true);
      }
      format = {
        tag,
        channels: view.getUint16(body + 2, true),
        sampleRate: view.getUint32(body + 4, true),
        bits: view.getUint16(body + 14, true),
      };
    } else if (id === 0x64617461) {
      data = { offset: body, size: Math.min(size, view.byteLength - body) };
    }
    offset = body + size + (size % 2);
  }
  if (format === null || data === null || format.channels === 0) {
    return null;
  }
  const bytesPerSample = format.bits / 8;
  if (!Number.isInteger(bytesPerSample) || bytesPerSample === 0) {
    return null;
  }
  const frames = Math.floor(data.size / (bytesPerSample * format.channels));
  const stride = bytesPerSample * format.channels;
  const samples = new Float32Array(frames);
  // The scaling matches decode-wav: integers divide by the format's full scale
  // and floats are only clamped.
  const scale = 2 ** (format.bits - 1);
  for (let frame = 0; frame < frames; frame += 1) {
    const at = data.offset + frame * stride;
    let value;
    if (format.tag === 3) {
      const raw = format.bits === 64 ? view.getFloat64(at, true) : view.getFloat32(at, true);
      value = Math.min(1, Math.max(-1, raw));
    } else if (format.tag !== 1) {
      return null;
    } else if (format.bits === 8) {
      value = (view.getUint8(at) - 128) / scale;
    } else if (format.bits === 16) {
      value = view.getInt16(at, true) / scale;
    } else if (format.bits === 24) {
      value = ((view.getUint8(at) | (view.getUint8(at + 1) << 8) | (view.getInt8(at + 2) << 16)) /
        scale);
    } else if (format.bits === 32) {
      value = view.getInt32(at, true) / scale;
    } else {
      return null;
    }
    samples[frame] = value;
  }
  return { sampleRate: format.sampleRate, channels: format.channels, samples };
}

async function readAudio(bytes) {
  const wav = readWav(bytes);
  if (wav !== null) {
    return wav;
  }
  const context = new AudioContext();
  try {
    const buffer = await context.decodeAudioData(bytes.slice(0));
    return {
      sampleRate: buffer.sampleRate,
      channels: buffer.numberOfChannels,
      samples: buffer.getChannelData(0),
      resampled: true,
    };
  } finally {
    context.close();
  }
}

async function decodeFile(file) {
  if (session.decoding) {
    return;
  }
  session.decoding = true;
  stopMicrophone();
  clearError();
  try {
    const audio = await readAudio(await file.arrayBuffer());
    const samples = audio.samples;
    const duration = samples.length / audio.sampleRate;
    replaceReceiver(audio.sampleRate, false, duration + FILE_STAGING_MARGIN_SECONDS);
    appendLog([
      `${file.name}: ${audio.sampleRate} Hz, ${audio.channels} channel(s), ` +
        `${duration.toFixed(2)} s` +
        (audio.resampled === true ? ' (resampled by the browser)' : ''),
    ]);
    for (let offset = 0; offset < samples.length; offset += FILE_CHUNK_SAMPLES) {
      const end = Math.min(offset + FILE_CHUNK_SAMPLES, samples.length);
      session.receiver.push(samples.subarray(offset, end));
      if ((offset / FILE_CHUNK_SAMPLES) % FILE_YIELD_EVERY === 0) {
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
    }
    finishReception();
  } catch (error) {
    showError(error);
  } finally {
    session.decoding = false;
  }
}

async function startMicrophone() {
  clearError();
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
        channelCount: 1,
      },
    });
  } catch (error) {
    showError(error);
    return;
  }
  try {
    const context = new AudioContext();
    await context.audioWorklet.addModule(new URL('./capture-worklet.js', import.meta.url));
    await context.resume();
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, 'capture', {
      processorOptions: { blockSamples: MIC_BLOCK_SAMPLES },
    });
    // Some engines never call process() on a node whose output goes nowhere.
    const sink = context.createGain();
    sink.gain.value = 0;
    source.connect(node).connect(sink).connect(context.destination);

    replaceReceiver(context.sampleRate, true, MIC_STAGING_SECONDS);
    node.port.onmessage = (event) => {
      if (session.mic === null) {
        return;
      }
      try {
        session.receiver.push(event.data);
      } catch (error) {
        showError(error);
        stopMicrophone();
      }
    };
    session.mic = { context, stream, node, source, sink };
    appendLog([`microphone open at ${context.sampleRate} Hz`]);
    ui.micStart.disabled = true;
    ui.micStop.disabled = false;
  } catch (error) {
    for (const track of stream.getTracks()) {
      track.stop();
    }
    showError(error);
  }
}

function stopMicrophone() {
  if (session.mic === null) {
    return;
  }
  const { context, stream, node, source, sink } = session.mic;
  session.mic = null;
  node.port.onmessage = null;
  source.disconnect();
  node.disconnect();
  sink.disconnect();
  for (const track of stream.getTracks()) {
    track.stop();
  }
  context.close();
  ui.micStart.disabled = false;
  ui.micStop.disabled = true;
  try {
    finishReception();
  } catch (error) {
    appendLog([String(error?.message ?? error)]);
  }
}

ui.file.addEventListener('change', () => {
  const file = ui.file.files?.[0];
  if (file !== undefined) {
    decodeFile(file);
  }
});

ui.micStart.addEventListener('click', startMicrophone);
ui.micStop.addEventListener('click', stopMicrophone);

ui.reset.addEventListener('click', () => {
  if (session.receiver === null) {
    return;
  }
  clearError();
  session.receiver.reset();
  session.painted = -1;
  session.completedAt = null;
  session.rows = 0;
  session.rowsAt = performance.now();
  ui.log.replaceChildren();
  context2d.clearRect(0, 0, ui.canvas.width, ui.canvas.height);
});

ui.download.addEventListener('click', () => {
  ui.canvas.toBlob((blob) => {
    if (blob === null) {
      return;
    }
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${(ui.mode.textContent ?? 'sstv').replace(/\s+/g, '')}.png`;
    link.click();
    URL.revokeObjectURL(url);
  }, 'image/png');
});

for (const name of ['dragenter', 'dragover']) {
  document.addEventListener(name, (event) => {
    event.preventDefault();
    document.body.classList.add('dragging');
  });
}

for (const name of ['dragleave', 'drop']) {
  document.addEventListener(name, (event) => {
    event.preventDefault();
    document.body.classList.remove('dragging');
  });
}

document.addEventListener('drop', (event) => {
  const file = event.dataTransfer?.files?.[0];
  if (file !== undefined) {
    decodeFile(file);
  }
});
