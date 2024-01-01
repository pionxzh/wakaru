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

    getMeasurement(): Measurement {
        return this.collected
    }

    merge(...timing: Timing[]) {
        this.collected.push(...timing.flatMap(t => t.collected))
    }
}

/* eslint-disable node/prefer-global/process */
function hrtime(start?: [number, number]): [number, number] {
    if (typeof process !== 'undefined' && typeof process.hrtime === 'function') return process.hrtime(start)

    // browser polyfill
    const clockTime = performance.now() * 1e-3
    let seconds = Math.floor(clockTime)
    let nanoseconds = Math.floor((clockTime % 1) * 1e9)

    if (start) {
        seconds -= start[0]
        nanoseconds -= start[1]
        if (nanoseconds < 0) {
            seconds -= 1
            nanoseconds += 1e9
        }
    }

    return [seconds, nanoseconds]
}
/* eslint-enable node/prefer-global/process */
