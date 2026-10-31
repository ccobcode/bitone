// 提示音.现场合成:不需要音频文件,CSP 也没有放开媒体来源.音量压低,办公环境里不突兀.
export type Chime = "incoming" | "included" | "failed";

const VOLUME = 0.06;
const SILENCE = 0.0001;

const NOTES: Record<Chime, readonly { hz: number; ms: number }[]> = {
    incoming: [
        { hz: 660, ms: 90 },
        { hz: 880, ms: 90 },
    ],
    included: [{ hz: 880, ms: 120 }],
    failed: [
        { hz: 330, ms: 90 },
        { hz: 262, ms: 90 },
    ],
};

let context: AudioContext | null = null;

/** 浏览器要求用户点过页面才能发声,所以要在点击时调用. */
export function unlockChime(): void {
    context ??= new AudioContext();
    void context.resume();
}

export function playChime(chime: Chime): void {
    if (context?.state !== "running") return;
    let at = context.currentTime;
    for (const { hz, ms } of NOTES[chime]) {
        const end = at + ms / 1000;
        const tone = context.createOscillator();
        const gain = context.createGain();
        tone.frequency.value = hz;
        gain.gain.setValueAtTime(VOLUME, at);
        gain.gain.exponentialRampToValueAtTime(SILENCE, end);
        tone.connect(gain).connect(context.destination);
        tone.start(at);
        tone.stop(end);
        at = end;
    }
}
