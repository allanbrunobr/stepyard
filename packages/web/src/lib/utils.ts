import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatUsd(value: number): string {
  return `$${value.toFixed(2)}`;
}

export function formatNumber(value: number): string {
  return value.toLocaleString();
}
