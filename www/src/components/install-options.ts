export const installOptions = [
  {
    id: "npm",
    label: "npm",
    command: "npx grida fx init my-assets",
  },
  {
    id: "python",
    label: "Python",
    command: "pip install grida && python -m grida.fx init my-assets",
  },
] as const;
