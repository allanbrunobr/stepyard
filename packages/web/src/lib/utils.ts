import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatTokens(n: number): string {
  return n.toLocaleString();
}

export function formatCost(n: number): string {
  return `$${n.toFixed(2)}`;
}
