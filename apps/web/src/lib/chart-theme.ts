/**
 * Recharts theme — Maelo palette. Sharp axes, no rounded gradients,
 * editorial sparse data-ink ratio.
 */

export const chartColors = {
  green: "#505E4D",
  greenDark: "#4B6B5A",
  greenMid: "#6B7A63",
  cream: "#f7f6da",
  orange: "#c67e5b",
  orangeDeep: "#b86843",
  ink: "#1c241c",
  muted: "#646e64",
  border: "#cfd5c8",
  danger: "#b04545",
};

export const chartAxisProps = {
  fontSize: 11,
  tickLine: false,
  axisLine: { stroke: chartColors.border },
  tick: { fill: chartColors.muted, fontFamily: "Inter" },
};

export const chartTooltipStyle = {
  background: "white",
  border: `1px solid ${chartColors.border}`,
  borderRadius: 0, // sharp corners — Maelo
  fontFamily: "Inter",
  fontSize: 12,
  padding: 8,
  boxShadow: "0 4px 12px rgba(28,36,28,0.06)",
};

export const chartGradients = {
  green: {
    id: "green-grad",
    from: chartColors.green,
    to: chartColors.green + "00",
  },
  orange: {
    id: "orange-grad",
    from: chartColors.orange,
    to: chartColors.orange + "00",
  },
  danger: {
    id: "danger-grad",
    from: chartColors.danger,
    to: chartColors.danger + "00",
  },
};

/** Categorical palette — green-first, then orange accent, then mid tones. */
export const categoricalPalette = [
  chartColors.green,
  chartColors.orange,
  chartColors.greenMid,
  chartColors.orangeDeep,
  chartColors.greenDark,
  chartColors.muted,
];
