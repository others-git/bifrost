// Shared jsdom setup for component tests.
//
// jsdom ships no `matchMedia`, and this app asks for it on sight — `useViewport`
// is the responsive switch behind nearly every component, and the animated
// surfaces check `prefers-reduced-motion`. Without the stub a component test
// fails inside an effect with a bare "window.matchMedia is not a function",
// which reads as a broken test rather than a missing browser API.
//
// It answers "no match" to everything, i.e. a desktop viewport with motion
// allowed. A test that needs another viewport should override this itself.

if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = (query: string): MediaQueryList =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}
