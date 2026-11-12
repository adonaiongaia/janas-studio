/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        ink: {
          950: "#07080a",
          900: "#0b0d11",
          850: "#0f1218",
          800: "#141821",
          750: "#181d28",
          700: "#1d2431",
          600: "#2a3342",
          500: "#3a4558",
        },
        acid: {
          200: "#efffc2",
          300: "#e2ff8f",
          400: "#cdf75a",
          500: "#b3e63c",
          600: "#8fbf22",
          700: "#6d9418",
        },
      },
      fontFamily: {
        sans: [
          "Inter",
          "ui-sans-serif",
          "system-ui",
          "-apple-system",
          "Segoe UI",
          "Roboto",
          "Helvetica Neue",
          "Arial",
          "sans-serif",
        ],
        mono: [
          "JetBrains Mono",
          "ui-monospace",
          "SFMono-Regular",
          "Menlo",
          "Consolas",
          "Liberation Mono",
          "monospace",
        ],
      },
      boxShadow: {
        panel: "0 18px 50px -24px rgba(0, 0, 0, 0.85)",
        glow: "0 0 30px -8px rgba(205, 247, 90, 0.35)",
      },
      keyframes: {
        "pulse-soft": {
          "0%, 100%": { opacity: "1" },
          "50%": { opacity: "0.35" },
        },
        shimmer: {
          "100%": { transform: "translateX(100%)" },
        },
        "fade-in": {
          from: { opacity: "0", transform: "translateY(4px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
      },
      animation: {
        "pulse-soft": "pulse-soft 1.8s ease-in-out infinite",
        shimmer: "shimmer 1.8s infinite",
        "fade-in": "fade-in 220ms ease-out",
      },
    },
  },
  plugins: [],
};
