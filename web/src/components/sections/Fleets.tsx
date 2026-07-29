import { Fleet } from "@/components/Fleet";
import type { Conductor } from "@/lib/conductor";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Fleets({ reduceMotion, conductor }: Props) {
  return (
    <section className="section fleets" id="fleets">
      <p className="section-label">Parallel agents</p>
      <h2>Many named instances. Same definition.</h2>
      <p>
        Each agent gets its own name so graphs do not collide. Prefer a worktree
        per agent. Fleets share the lifecycle contract, not state.
      </p>
      <Fleet reduceMotion={reduceMotion} conductor={conductor} />
    </section>
  );
}
