// From https://github.com/fabiospampinato/promise-concurrency-limiter
// with MIT license
export class Concurrency {
    private concurrency: number
    private count: number
    private queue: Set<() => void>

    constructor(options: { concurrency: number }) {
        this.concurrency = options.concurrency
        this.count = 0
        this.queue = new Set()
    }

    public add<T>(fn: () => Promise<T>): Promise<T> {
        if (this.count < this.concurrency) {
            return this.run(fn)
        }

        return new Promise<T>((resolve) => {
            const callback = (): void => resolve(this.run(fn))

            this.queue.add(callback)
        })
    }

    private flush(): void {
        for (const callback of this.queue) {
            if (this.count >= this.concurrency) break

            this.queue.delete(callback)

            callback()
        }
    }

    private run<T>(fn: () => Promise<T>): Promise<T> {
        this.count++

        const promise = fn()

        const cleanup = (): void => {
            this.count--

            this.flush()
        }

        promise.then(cleanup, cleanup)

        return promise
    }
}
