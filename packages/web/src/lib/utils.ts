import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatTokens(n: number): string {
  return n.toLocaleString();
}

export function formatNumber(value: number): string {
  return value.toLocaleString();
}

export function formatCost(n: number): string {
  return `$${n.toFixed(2)}`;
}

export function formatUsd(value: number): string {
  return `$${value.toFixed(2)}`;
}
