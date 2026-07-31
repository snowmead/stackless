import { LayerWall } from "@/components/LayerWall";

type Props = {
  reduceMotion: boolean;
};

export function Layer({ reduceMotion }: Props) {
  return (
    <section className="section layer" id="layer">
      <p className="section-label">The layer</p>
      <h2>Agents run & test in parallel</h2>
      <p>
        Fan your end-to-end suite out to a fleet of agents. Each one crosses
        the same wall — <code>stackless up</code> with its own name — and
        Stripe Projects provisions an isolated environment behind it: hosts,
        integrations, secrets. Specs run concurrently against real backends,
        and every instance burns down on lease.
      </p>
      <LayerWall reduceMotion={reduceMotion} />
    </section>
  );
}
