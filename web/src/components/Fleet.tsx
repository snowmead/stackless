import { useEffect, useRef } from "react";

import {
  applyCues,
  FLEET_MS,
  FLEET_OFFSET,
  type Conductor,
  type Cue,
} from "@/lib/conductor";

const LANES = [
  {
    phase: 0,
    code: "feat/auth",
    tag: "worktree",
    cmd: "stackless up --name agent-a --on local --json",
  },
  {
    phase: 1,
    code: "pr-42",
    tag: "branch",
    cmd: "stackless up --name pr-42 --on vercel --json",
  },
  {
    phase: 2,
    code: "local · dirty",
    tag: "--dirty",
    cmd: "stackless up --name dirty-1 --on local --source web=. --dirty --json",
  },
] as const;

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Fleet({ reduceMotion, conductor }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const lanes = Array.from(
      root.querySelectorAll<HTMLElement>("[data-fleet-lane]"),
    );
    const fired = lanes.map(() => new Set<number>());
    const prevLocal = lanes.map(() => -1);

    const laneSchedule: Cue<HTMLElement>[] = [
      {
        at: 0,
        run: (lane) => {
          lane.dataset.state = "idle";
        },
      },
      {
        at: 480,
        run: (lane) => {
          lane.dataset.state = "up";
        },
      },
      {
        at: 1400,
        run: (lane) => {
          lane.dataset.state = "alive";
        },
      },
      {
        at: 3600,
        run: (lane) => {
          lane.dataset.state = "down";
        },
      },
      {
        at: 4800,
        run: (lane) => {
          lane.dataset.state = "idle";
        },
      },
    ];

    if (reduceMotion) {
      lanes.forEach((lane) => {
        lane.dataset.state = "alive";
      });
      return;
    }

    return conductor.on((t) => {
      lanes.forEach((lane, i) => {
        const phase = Number(lane.dataset.phase || i);
        const local = (t + phase * FLEET_OFFSET) % FLEET_MS;
        if (local < prevLocal[i]) {
          fired[i].clear();
        }
        prevLocal[i] = local;
        applyCues(laneSchedule, local, fired[i], lane);
      });
    });
  }, [conductor, reduceMotion]);

  return (
    <div
      ref={rootRef}
      className="fleet"
      id="fleet"
      aria-label="Three parallel agent lanes spawning and tearing down stacks"
    >
      <div className="fleet-spine">
        <span className="fleet-spine-label">stackless.toml</span>
      </div>
      <div className="fleet-lanes">
        {LANES.map((lane) => (
          <article
            key={lane.code}
            className="fleet-lane"
            data-fleet-lane
            data-phase={lane.phase}
          >
            <header className="fleet-lane-head">
              <code>{lane.code}</code>
              <span>{lane.tag}</span>
            </header>
            <p className="fleet-cmd">{lane.cmd}</p>
            <ul className="fleet-nodes">
              <li data-fleet-node style={{ ["--i" as string]: 0 }}>
                web
              </li>
              <li data-fleet-node style={{ ["--i" as string]: 1 }}>
                clerk
              </li>
              <li data-fleet-node style={{ ["--i" as string]: 2 }}>
                db
              </li>
            </ul>
          </article>
        ))}
      </div>
    </div>
  );
}
