import { Footer } from "@/components/Footer";
import { Hero } from "@/components/Hero";
import { Sections } from "@/components/Sections";
import { useReducedMotion } from "@/hooks/use-reduced-motion";

export default function App() {
  const reduceMotion = useReducedMotion();

  return (
    <div className="page">
      <Hero reduceMotion={reduceMotion} />
      <Sections reduceMotion={reduceMotion} />
      <Footer />
    </div>
  );
}
