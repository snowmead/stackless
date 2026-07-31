/** Prompt copied by the hero "Copy Prompt" CTA — paste into a coding agent. */
export const COPY_PROMPT =
  "Read https://stackless.sh/start.md then install the stackless CLI and add the stackless skill...";

export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    document.body.removeChild(area);
  }
}
