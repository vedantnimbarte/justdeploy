// Technology marks for templates, from Simple Icons (CC0). Named imports keep
// only these icons in the bundle.

import {
  siAdminer,
  siExpress,
  siFastapi,
  siGo,
  siHtml5,
  siMariadb,
  siMongodb,
  siMysql,
  siN8n,
  siNextdotjs,
  siPostgresql,
  siReact,
  siRedis,
  siRust,
  siUmami,
  siUptimekuma,
  siVite,
  type SimpleIcon,
} from "simple-icons";
import { cx } from "./ui";

/** The marks for each template, in the order its services matter. */
const TEMPLATE_ICONS: Record<string, SimpleIcon[]> = {
  "vite-react": [siReact, siVite],
  nextjs: [siNextdotjs],
  "static-site": [siHtml5],
  "express-api": [siExpress],
  fastapi: [siFastapi],
  "go-api": [siGo],
  "rust-axum": [siRust],
  "fastapi-postgres": [siFastapi, siPostgresql],
  "express-redis": [siExpress, siRedis],
  "uptime-kuma": [siUptimekuma],
  n8n: [siN8n],
  umami: [siUmami],
  adminer: [siAdminer],
  postgres: [siPostgresql],
  mysql: [siMysql],
  mariadb: [siMariadb],
  redis: [siRedis],
  mongodb: [siMongodb],
};

export function templateIcons(id: string): SimpleIcon[] {
  return TEMPLATE_ICONS[id] ?? [];
}

/** Relative luminance of a brand hex, to catch marks that vanish on dark surfaces. */
function luminance(hex: string): number {
  const channel = (i: number) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

/**
 * A brand mark on a soft tint of its own colour. Near-black brands take the
 * text colour instead, so they stay visible in dark mode.
 */
export function BrandIcon({ icon, className }: { icon: SimpleIcon; className?: string }) {
  const dark = luminance(icon.hex) < 0.12;
  const color = dark ? "var(--jd-fg)" : `#${icon.hex}`;
  return (
    <span
      className={cx("inline-flex size-10 shrink-0 items-center justify-center rounded-control", className)}
      style={{ background: dark ? "var(--jd-subtle)" : `color-mix(in srgb, #${icon.hex} 14%, transparent)` }}
      title={icon.title}
    >
      <svg viewBox="0 0 24 24" className="size-5" fill={color} role="img" aria-label={icon.title}>
        <path d={icon.path} />
      </svg>
    </span>
  );
}
