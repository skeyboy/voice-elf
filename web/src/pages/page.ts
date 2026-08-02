export interface Page {
  mount(root: HTMLElement): void | Promise<void>;
  destroy(): void | Promise<void>;
}
