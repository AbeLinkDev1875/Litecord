// LiteCord voice gate. Lets your voice through when it's loud enough (voice activity)
// or while your push-to-talk key is held. It runs on the audio thread, so it keeps
// working smoothly when the LiteCord window is in the background.
class VoiceGate extends AudioWorkletProcessor {
  constructor() {
    super();
    this.mode = "vad";      // "vad" (voice activity) or "ptt" (push to talk)
    this.auto = true;       // pick the sensitivity automatically from the background noise
    this.threshold = -50;   // manual sensitivity, in dB
    this.ptt = false;       // push-to-talk key held
    this.floor = -70;       // background noise estimate, in dB
    this.hold = 0;          // samples left before closing (so word endings aren't cut off)
    this.gain = 0;
    this.level = -100;
    this.sinceReport = 0;
    this.port.onmessage = e => {
      const d = e.data || {};
      for (const k of ["mode", "auto", "threshold", "ptt"]) if (k in d) this[k] = d[k];
    };
  }

  process(inputs, outputs) {
    const inp = inputs[0] && inputs[0][0], out = outputs[0] && outputs[0][0];
    if (!out) return true;
    if (!inp) { out.fill(0); return true; }

    let sum = 0;
    for (let i = 0; i < inp.length; i++) sum += inp[i] * inp[i];
    const db = 10 * Math.log10(sum / inp.length + 1e-12);
    this.level = Math.max(db, this.level - 0.6);  // the meter falls back slowly
    // Background noise: drops quickly to quiet moments, creeps up slowly (about 1 dB a second).
    this.floor = db < this.floor ? this.floor * 0.9 + db * 0.1 : this.floor + 0.0025;
    const threshold = this.auto ? Math.min(-28, Math.max(-62, this.floor + 15)) : this.threshold;

    let open;
    if (this.mode === "ptt") open = this.ptt;
    else {
      if (db > threshold) this.hold = sampleRate * 0.35;
      else this.hold = Math.max(0, this.hold - inp.length);
      open = this.hold > 0;
    }
    const target = open ? 1 : 0, step = open ? 0.02 : 0.0006;  // open fast, fade out gently
    for (let i = 0; i < inp.length; i++) {
      this.gain += (target - this.gain) * step;
      out[i] = inp[i] * this.gain;
    }

    this.sinceReport += inp.length;
    if (this.sinceReport >= sampleRate / 25) {  // tell the page about 25 times a second
      this.sinceReport = 0;
      this.port.postMessage({level: this.level, threshold, open});
    }
    return true;
  }
}
registerProcessor("litecord-voice-gate", VoiceGate);
