import { Layer } from "@/components/sections/Layer";
import { Lifecycle } from "@/components/sections/Lifecycle";
import { Primitives } from "@/components/sections/Primitives";
import { Sdk } from "@/components/sections/Sdk";

type Props = {
  reduceMotion: boolean;
};

export function Sections({ reduceMotion }: Props) {
  return (
    <main>
      <Primitives reduceMotion={reduceMotion} />
      <Layer reduceMotion={reduceMotion} />
      <Lifecycle />
      <Sdk />
    </main>
  );
}
