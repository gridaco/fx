/** Navigation identities are recorded scope occurrences, not parsed step paths. */
export interface NavigationScope { id: string; title: string; parent: string | null }
export interface Breadcrumb { id: string | null; title: string }
export interface NavigationState { scope: string | null; breadcrumbs: Breadcrumb[]; selected: string | null }

/** Owns the current workflow and per-workflow selection independently of the canvas. */
export class WorkflowNavigation {
  private scopes = new Map<string, NavigationScope>();
  private selections = new Map<string | null, string | null>();
  private current: string | null = null;
  private title = "Workflow";

  setScopes(scopes: readonly NavigationScope[], rootTitle: string) {
    const previous = this.scopes;
    this.scopes = new Map(scopes.map((scope) => [scope.id, { ...scope }]));
    this.title = rootTitle;
    const visited = new Set<string>();
    while (this.current !== null && !this.scopes.has(this.current)) {
      if (visited.has(this.current)) { this.current = null; break; }
      visited.add(this.current);
      this.current = previous.get(this.current)?.parent ?? null;
    }
    for (const key of this.selections.keys()) if (key !== null && !this.scopes.has(key)) this.selections.delete(key);
  }

  getSnapshot(): NavigationState {
    const breadcrumbs: Breadcrumb[] = [];
    const visited = new Set<string>();
    let id = this.current;
    while (id !== null && !visited.has(id)) {
      visited.add(id);
      const scope = this.scopes.get(id);
      if (!scope) break;
      breadcrumbs.unshift({ id, title: scope.title });
      id = scope.parent;
    }
    breadcrumbs.unshift({ id: null, title: this.title });
    return { scope: this.current, breadcrumbs, selected: this.selections.get(this.current) ?? null };
  }

  /** Opening is limited to a child of the currently displayed workflow. */
  open(id: string): boolean {
    const scope = this.scopes.get(id);
    if (!scope || scope.parent !== this.current) return false;
    this.current = id;
    return true;
  }

  /** Breadcrumb navigation only permits the current scope or one of its ancestors. */
  goTo(id: string | null): boolean {
    if (!this.getSnapshot().breadcrumbs.some((crumb) => crumb.id === id)) return false;
    this.current = id;
    return true;
  }

  select(id: string | null) { this.selections.set(this.current, id); }
}
