export const INSTALL_CMD =
  "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh";

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
