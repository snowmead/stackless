export type Cue<T = void> = {
  at: number;
  run: (ctx: T) => void;
};

export type ConductorListener = (t: number) => void;

export type Conductor = {
  on: (fn: ConductorListener) => () => void;
  start: () => void;
  pause: () => void;
  resume: () => void;
  stop: () => void;
};

export function createConductor(): Conductor {
  let t0 = 0;
  let paused = false;
  let pauseAccum = 0;
  let pauseStarted = 0;
  let raf = 0;
  let running = false;
  const listeners = new Set<ConductorListener>();

  function now() {
    if (paused) {
      return pauseStarted - t0 - pauseAccum;
    }
    return performance.now() - t0 - pauseAccum;
  }

  return {
    on(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    start() {
      if (running) return;
      running = true;
      t0 = performance.now();
      pauseAccum = 0;
      paused = false;
      const tick = () => {
        const t = now();
        for (const fn of listeners) fn(t);
        raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    },
    pause() {
      if (paused) return;
      paused = true;
      pauseStarted = performance.now();
    },
    resume() {
      if (!paused) return;
      pauseAccum += performance.now() - pauseStarted;
      paused = false;
    },
    stop() {
      running = false;
      cancelAnimationFrame(raf);
    },
  };
}

export function applyCues<T>(
  schedule: Cue<T>[],
  localT: number,
  fired: Set<number>,
  ctx: T,
) {
  for (let i = 0; i < schedule.length; i += 1) {
    const cue = schedule[i];
    if (localT >= cue.at && !fired.has(i)) {
      fired.add(i);
      cue.run(ctx);
    }
  }
}

export const NODE_ORDER = ["clerk", "web", "db"] as const;
export const HERO_MS = 10000;
export const FLEET_MS = 6000;
export const FLEET_OFFSET = 2000;
