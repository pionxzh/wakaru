import { clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'
import type { ClassValue } from 'clsx'

export const noop = () => {}

export const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms))

export function cn(...inputs: ClassValue[]) {
    return twMerge(clsx(inputs))
}
