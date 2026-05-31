import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "rgb(var(--bg) / <alpha-value>)",
        fg: "rgb(var(--fg) / <alpha-value>)",
        muted: "rgb(var(--muted) / <alpha-value>)",
        border: "rgb(var(--border) / <alpha-value>)",
        card: "rgb(var(--card) / <alpha-value>)",
        accent: "rgb(var(--accent) / <alpha-value>)",
        cta: "rgb(var(--cta) / <alpha-value>)",
        "cta-hover": "rgb(var(--cta-hover) / <alpha-value>)",
        success: "rgb(var(--success) / <alpha-value>)",
        warning: "rgb(var(--warning) / <alpha-value>)",
        danger: "rgb(var(--danger) / <alpha-value>)",
        maelo: {
          DEFAULT: "rgb(var(--maelo-bg) / <alpha-value>)",
          green: "rgb(var(--maelo-green) / <alpha-value>)",
          mid: "rgb(var(--maelo-green-mid) / <alpha-value>)",
          cream: "rgb(var(--maelo-cream) / <alpha-value>)",
          orange: "rgb(var(--maelo-orange) / <alpha-value>)",
          "orange-deep": "rgb(var(--maelo-orange-deep) / <alpha-value>)",
          ink: "rgb(var(--maelo-ink) / <alpha-value>)",
        },
      },
      fontFamily: {
        sans: ["Inter", "ui-sans-serif", "system-ui", "sans-serif"],
        mono: ["DM Mono", "ui-monospace", "Menlo", "monospace"],
      },
      borderRadius: {
        none: "0",
        sm: "2px",
        DEFAULT: "0", // sharp by default
        md: "4px",
        lg: "6px",
      },
    },
  },
  plugins: [],
} satisfies Config;
