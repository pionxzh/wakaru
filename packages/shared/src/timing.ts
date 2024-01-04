export interface TimingStat {
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

export class Timing {
    private stats: TimingStat[] = []

    startMeasure(filename: string, key: string) {
        const _stop = this.start()
        const stop = () => {
            const time = _stop()
            this.stats.push({ filename, key, time })
        }

        return stop
    }

    start() {
        const start = hrtime()

        const stop = () => {
            const end = hrtime(start)
            const time = end[0] * 1e3 + end[1] / 1e6
            return time
        }

        return stop
    }

    getMeasurement(): TimingStat[] {
        return this.stats
    }

    merge(...timing: Timing[]) {
        this.stats.push(...timing.flatMap(t => t.stats))
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
