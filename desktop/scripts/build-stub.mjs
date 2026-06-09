import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "dist");

await mkdir(dist, { recursive: true });
await writeFile(
  join(dist, "index.html"),
  `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <title>Penpot Desktop</title>
  </head>
  <body>
    <p>Penpot Desktop shell — the main window loads your Penpot instance directly.</p>
  </body>
</html>
`,
);
await copyFile(join(root, "settings.html"), join(dist, "settings.html"));
