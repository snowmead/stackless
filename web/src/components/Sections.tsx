import { Authoring } from "@/components/sections/Authoring";
import { Fleets } from "@/components/sections/Fleets";
import { Install } from "@/components/sections/Install";
import { Layer } from "@/components/sections/Layer";
import { Lifecycle } from "@/components/sections/Lifecycle";
import { MachineContract } from "@/components/sections/MachineContract";
import { Primitives } from "@/components/sections/Primitives";
import { Sdk } from "@/components/sections/Sdk";
import { Substrates } from "@/components/sections/Substrates";
import type { Conductor } from "@/lib/conductor";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Sections({ reduceMotion, conductor }: Props) {
  return (
    <main>
      <Primitives />
      <Layer reduceMotion={reduceMotion} />
      <Lifecycle />
      <Substrates />
      <MachineContract />
      <Sdk />
      <Fleets reduceMotion={reduceMotion} conductor={conductor} />
      <Authoring />
      <Install />
    </main>
  );
}
