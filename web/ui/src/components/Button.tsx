import { createLink } from "@tanstack/react-router";
import { forwardRef, type AnchorHTMLAttributes, type ButtonHTMLAttributes } from "react";

import styles from "./Button.module.css";

type Variant = "default" | "primary" | "ghost";
type Size = "md" | "sm";

interface Look {
  variant?: Variant;
  size?: Size;
  /** Square button holding only an icon (give it an aria-label). */
  icon?: boolean;
}

function className({ variant = "default", size = "md", icon }: Look, extra?: string) {
  return [styles.btn, styles[variant], size === "sm" && styles.sm, icon && styles.icon, extra]
    .filter(Boolean)
    .join(" ");
}

export const Button = forwardRef<HTMLButtonElement, ButtonHTMLAttributes<HTMLButtonElement> & Look>(
  function Button({ variant, size, icon, className: extra, type = "button", ...rest }, ref) {
    return <button ref={ref} type={type} className={className({ variant, size, icon }, extra)} {...rest} />;
  },
);

const Anchor = forwardRef<HTMLAnchorElement, AnchorHTMLAttributes<HTMLAnchorElement> & Look>(
  function Anchor({ variant, size, icon, className: extra, ...rest }, ref) {
    return <a ref={ref} className={className({ variant, size, icon }, extra)} {...rest} />;
  },
);

/** A router link that looks like a button; takes the same `to`/`params` as `Link`. */
export const ButtonLink = createLink(Anchor);
