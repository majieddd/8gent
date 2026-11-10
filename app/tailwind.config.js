/** kr8's design values: its neutral palette as CSS variables (src/styles.css), Inter and JetBrains
 *  Mono, a 10 px base radius, and its motion timings. */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        card: "var(--card)",
        fg: "var(--fg)",
        muted: "var(--muted)",
        line: "var(--border)",
        soft: "var(--soft)",
        primary: "var(--primary)",
        "on-primary": "var(--on-primary)",
        ok: "var(--ok)",
        warn: "var(--warn)",
        bad: "var(--bad)",
        accent: "var(--accent)",
        track: "var(--track)",
        chrome: { fg: "var(--chrome-fg)", muted: "var(--chrome-muted)", sel: "var(--chrome-sel)" },
      },
      fontFamily: {
        sans: ['"Inter Variable"', "Inter", '"Avenir Next"', '"Segoe UI"', "sans-serif"],
        mono: ['"JetBrains Mono"', "ui-monospace", "monospace"],
      },
      borderRadius: { lg: "10px", md: "8px", sm: "6px", xl: "12px", "2xl": "16px" },
      transitionTimingFunction: { kr8: "cubic-bezier(.25,1,.5,1)" },
      transitionDuration: { fast: "120ms", base: "180ms", slow: "240ms" },
    },
  },
  plugins: [],
};
