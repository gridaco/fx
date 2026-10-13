export interface ViewerRoute {
  api_base: string;
  project_id: string | null;
  entry_id: string | null;
  kind: "root" | "run" | "plan";
}

const identifier = "[a-f0-9]{64}";
const entryPath = new RegExp(`^/p/(${identifier})/(runs|plans)/(${identifier})/?$`);
const entryApi = new RegExp(`^/p/${identifier}/(?:runs|plans)/${identifier}/api$`);

/** Only canonical same-origin viewer routes can select an API or artifact scope. */
export function parseViewerRoute(pathname: string): ViewerRoute {
  if (pathname === "/") return { api_base: "/api", project_id: null, entry_id: null, kind: "root" };
  const match = entryPath.exec(pathname);
  if (!match) throw new Error("This is not a supported FX viewer address.");
  return {
    api_base: `/p/${match[1]}/${match[2]}/${match[3]}/api`,
    project_id: match[1], entry_id: match[3], kind: match[2] === "runs" ? "run" : "plan",
  };
}

export function validateViewerApiBase(apiBase: string): string {
  if (apiBase !== "/api" && !entryApi.test(apiBase)) throw new Error("This is not a supported FX viewer API address.");
  return apiBase;
}

export function viewerEntryUrl(projectId: string, kind: "run" | "plan", id: string): string {
  const path = `/p/${projectId}/${kind === "run" ? "runs" : "plans"}/${id}/`;
  parseViewerRoute(path);
  return path;
}
