import { hrtime } from 'node:process'

interface TimingStat {
    filename: string
    /**
     * Timing measurement key
     */
    key: string
    /**
     * Time in milliseconds
     */
    time: number
}

export type Measurement = TimingStat[]

export class Timing {
    private collected: TimingStat[] = []

    constructor(private enabled: boolean = true) { }

    /**
     * Collect a timing measurement
     */
    collect<T>(filename: string, key: string, fn: () => T): T {
        if (!this.enabled) return fn()

        const { result, time } = this.measureTime(fn)
        this.collected.push({ filename, key, time })

        return result
    }

    /**
     * Collect a timing measurement
     */
    async collectAsync<T>(filename: string, key: string, fn: () => T): Promise<T> {
        if (!this.enabled) return fn()

        const { result, time } = await this.measureTimeAsync(fn)
        this.collected.push({ filename, key, time })

        return result
    }

    /**
     * Measure the time it takes to execute a function
     */
    measureTime<T>(fn: () => T) {
        const start = hrtime()
        const result = fn()
        const end = hrtime(start)
        const time = end[0] * 1e3 + end[1] / 1e6

        return { result, time }
    }

    /**
     * Measure the time it takes to execute a async function
     */
    async measureTimeAsync<T>(fn: () => T) {
        const start = hrtime()
        const result = await fn()
        const end = hrtime(start)
        const time = end[0] * 1e3 + end[1] / 1e6

        return { result, time }
    }

    getMeasurements(): Measurement {
        return this.collected
    }
}
