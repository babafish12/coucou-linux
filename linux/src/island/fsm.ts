// Island open/close state machine.
// No DOM, no Tauri: it only reports transitions.

export type FsmState = "hidden" | "petit" | "home" | "coucou";

export class IslandStateMachine {
  state: FsmState = "hidden";

  onTransition: ((from: FsmState, to: FsmState) => void) | null = null;
  onDeadlineChanged: (() => void) | null = null;

  /** home → petit delay, seconds. */
  homeToPetitDelay = 15;
  activationMode: "hover" | "click" = "hover";
  autoHideEnabled = false;
  autoHideInterval = 30;
  /** coucou → petit once the greeting animation ends (no hover). */
  greetAutoCollapseDelay = 0.6;
  /** coucou → petit while the mouse hovers the greeting. */
  greetHoverCollapseDelay = 10;
  /** An alert waiting for an answer stays open, even when the mouse leaves. */
  pinned = false;

  private homeCollapse: number | null = null;
  private homeDeadline: number | null = null;
  private greetCollapse: number | null = null;
  private petitHide: number | null = null;
  private pointerInside = false;
  private autoHidden = false;

  configure(preferences: {
    activationMode: "hover" | "click";
    autoCloseInterval: number;
    autoHideEnabled: boolean;
    autoHideInterval: number;
  }) {
    const closeChanged = this.homeToPetitDelay !== preferences.autoCloseInterval;
    const hideChanged = this.autoHideEnabled !== preferences.autoHideEnabled ||
      this.autoHideInterval !== preferences.autoHideInterval;
    this.activationMode = preferences.activationMode;
    this.homeToPetitDelay = preferences.autoCloseInterval;
    this.autoHideEnabled = preferences.autoHideEnabled;
    this.autoHideInterval = preferences.autoHideInterval;
    if (!this.autoHideEnabled) this.autoHidden = false;
    if (closeChanged && this.state === "home" && !this.pointerInside) this.scheduleHomeCollapse();
    if (hideChanged) this.schedulePetitHide();
  }

  /** Scheduled home → petit time in performance.now() milliseconds. */
  get homeCollapseDeadline(): number | null {
    return this.homeDeadline;
  }

  // ── Inputs ──────────────────────────────────────────────────────────────────

  launch() {
    this.autoHidden = false;
    this.cancelTimers();
    this.transition("coucou");
  }

  mouseEntered() {
    this.autoHidden = false;
    this.pointerInside = true;
    this.clear("petitHide");
    switch (this.state) {
      case "hidden":
      case "petit":
        this.cancelTimers();
        this.transition(this.activationMode === "hover" ? "home" : "petit");
        break;
      case "home":
        this.clear("homeCollapse");
        break;
      case "coucou":
        this.scheduleGreetCollapse(this.greetHoverCollapseDelay);
        break;
    }
  }

  mouseLeft() {
    this.pointerInside = false;
    switch (this.state) {
      case "hidden":
        break;
      case "petit":
        this.schedulePetitHide();
        break;
      case "home":
        this.scheduleHomeCollapse();
        break;
      case "coucou":
        this.clear("greetCollapse");
        this.transition("petit");
        break;
    }
  }

  click() {
    if (this.state !== "petit" && this.state !== "hidden") return;
    this.autoHidden = false;
    this.cancelTimers();
    this.transition("home");
  }

  /** Greeting animation finished (T.end). Doesn't override a running hover timer. */
  greetComplete() {
    if (this.state !== "coucou") return;
    if (this.greetCollapse == null) this.scheduleGreetCollapse(this.greetAutoCollapseDelay);
  }

  /** Non-alert work event: show compact from hidden. */
  reveal() {
    if (this.state !== "hidden" || this.autoHidden) return;
    this.cancelTimers();
    this.transition("petit");
  }

  /** Alert or explicit request: open straight to expanded. */
  forceHome() {
    this.autoHidden = false;
    this.cancelTimers();
    this.transition("home");
  }

  /// Explicit close (OK button, Escape, an alert being answered).
  forcePetit() {
    this.autoHidden = false;
    this.cancelTimers();
    this.transition("petit");
  }

  /** Explicit pause; optional idle hiding uses the same native wake strip. */
  forceHidden() {
    this.autoHidden = false;
    this.cancelTimers();
    this.transition("hidden");
  }

  // ── Timers ──────────────────────────────────────────────────────────────────

  private schedulePetitHide() {
    this.clear("petitHide");
    if (!this.autoHideEnabled || this.state !== "petit" || this.pointerInside || this.pinned) return;
    this.petitHide = window.setTimeout(() => {
      this.petitHide = null;
      if (this.state === "petit" && !this.pointerInside && !this.pinned) {
        this.autoHidden = true;
        this.transition("hidden");
      }
    }, this.autoHideInterval * 1000);
  }

  private scheduleHomeCollapse() {
    if (this.pinned) {
      this.clear("homeCollapse");
      return;
    }
    if (this.homeCollapse != null) window.clearTimeout(this.homeCollapse);
    const delay = this.homeToPetitDelay * 1000;
    const deadline = performance.now() + delay;
    this.homeCollapse = window.setTimeout(() => {
      this.homeCollapse = null;
      this.setHomeDeadline(null);
      if (this.state === "home") this.transition("petit");
    }, delay);
    this.setHomeDeadline(deadline);
  }

  private setHomeDeadline(deadline: number | null) {
    if (this.homeDeadline === deadline) return;
    this.homeDeadline = deadline;
    this.onDeadlineChanged?.();
  }

  private scheduleGreetCollapse(delay: number) {
    this.clear("greetCollapse");
    this.greetCollapse = window.setTimeout(() => {
      this.greetCollapse = null;
      if (this.state === "coucou") this.transition("petit");
    }, delay * 1000);
  }

  private clear(which: "homeCollapse" | "greetCollapse" | "petitHide") {
    const id = this[which];
    if (id != null) window.clearTimeout(id);
    this[which] = null;
    if (which === "homeCollapse") this.setHomeDeadline(null);
  }

  cancelTimers() {
    this.clear("homeCollapse");
    this.clear("greetCollapse");
    this.clear("petitHide");
  }

  private transition(next: FsmState) {
    if (next === this.state) return;
    const from = this.state;
    this.state = next;
    if (next === "petit") this.schedulePetitHide();
    this.onTransition?.(from, next);
  }
}
