// Lint config with ONE job: the Rules of Hooks.
//
// A hook placed after an early `return` type-checks, builds, and then crashes
// the whole React tree at runtime with "Rendered fewer hooks than expected" —
// `tsc` and `vite build` cannot see it, and that is exactly how a broken Boards
// page shipped. This is the gate that does see it. Kept deliberately narrow:
// it is a correctness check, not a style opinion.
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default [
  {
    files: ["src/**/*.{ts,tsx}"],
    // The codebase carries `eslint-disable` comments for exhaustive-deps, a rule
    // this config deliberately does not run; don't report them as unused.
    linterOptions: { reportUnusedDisableDirectives: false },
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true }, sourceType: "module" },
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
    },
  },
];
