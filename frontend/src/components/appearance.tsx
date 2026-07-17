import { createContext, useContext } from "react";

/** Board-level "match the theme" appearance override. When true, widget accents
 * wear the theme's domain colours (cyan light / violet media / gold power)
 * instead of each lamp's actual chroma or the effect gradient — a board that
 * matches the appearance theme rather than reporting light state. Provided by a
 * board whose spec sets `match_theme`; everywhere else (Control, Rooms, Floor
 * Plan) it stays false, so the shared components behave exactly as before. */
export const MatchThemeContext = createContext(false);

export const useMatchTheme = () => useContext(MatchThemeContext);
