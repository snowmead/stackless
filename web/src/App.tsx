import { useEffect, useMemo } from "react";

import { Footer } from "@/components/Footer";
import { Hero } from "@/components/Hero";
import { Sections } from "@/components/Sections";
import { useReducedMotion } from "@/hooks/use-reduced-motion";
import { createConductor } from "@/lib/conductor";

export default function App() {
  const reduceMotion = useReducedMotion();
  const conductor = useMemo(() => createConductor(), []);

  useEffect(() => {
    if (reduceMotion) return;
    conductor.start();
    return () => conductor.stop();
  }, [conductor, reduceMotion]);

  return (
    <div className="page">
      <Hero reduceMotion={reduceMotion} conductor={conductor} />
      <Sections reduceMotion={reduceMotion} conductor={conductor} />
      <Footer />
    </div>
  );
}
