// Compile-time feature flags.
//
// The frontend twin of the backend's "code kept, off the menu" idiom (the
// unregistered `govee-lan`/`shelly`/`tasmota`/`wled` providers): a feature that
// isn't ready to be in front of people gets switched off here rather than
// deleted, so the work survives in-tree and comes back by flipping one line.
//
// A flag here is **not** a user setting — those live in `config` on the hub and
// arrive via `/api/settings` (`dev_mode` is one). This file is for things the
// product has decided not to ship yet, which no user should be able to turn on.
export const FEATURES: {
  /**
   * The 2D floor planner (`pages/FloorPlan.tsx`) — paint tiles/walls, place
   * devices, bind regions to Rooms.
   *
   * Off: it is an alternate visualization of control that already exists on
   * Dashboard / Rooms / Boards, and it carries the most surface area per unit of
   * use of anything in the app — a canvas, its own gesture handling, and the only
   * `isMobile`-specific layout left. Tabled rather than finished, so it is hidden
   * outright instead of sitting behind dev mode inviting half-maintenance.
   *
   * The page and its API still work; turning this back on restores the nav entry
   * and the route, and nothing else needs touching.
   */
  floorPlan: boolean;
} = {
  floorPlan: false,
};
