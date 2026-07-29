import { highlightCode, type CodeLang } from "@/lib/highlight";
import { cn } from "@/lib/utils";

type Props = {
  code: string;
  lang: CodeLang;
  className?: string;
  id?: string;
};

export function CodeBlock({ code, lang, className, id }: Props) {
  return (
    <pre
      className={cn("code-block code-block-hl", className)}
      id={id}
      tabIndex={0}
      data-lang={lang}
    >
      <code>{highlightCode(code, lang)}</code>
    </pre>
  );
}
