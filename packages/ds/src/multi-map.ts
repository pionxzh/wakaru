/**
 * A MultiMap data structure that allows multiple values for a single key.
 */
export class MultiMap<K, V> {
    private data: Map<K, Set<V>> = new Map()

    /**
     * Returns the set of values to which the specified key is mapped, or undefined if this map contains no mapping for the key.
     */
    get(key: K): Set<V> | undefined {
        return this.data.get(key)
    }

    /**
     * Associates the specified value with the specified key in this map.
     */
    set(key: K, value: V): void {
        let values = this.data.get(key)
        if (!values) {
            values = new Set()
            this.data.set(key, values)
        }
        values.add(value)
    }

    /**
     * Removes the association of the specified value with the specified key in this map.
     */
    remove(key: K, value: V): void {
        const values = this.data.get(key)
        if (values) {
            values.delete(value)
            if (values.size === 0) {
                this.data.delete(key)
            }
        }
    }

    /**
     * Returns true if this map contains a mapping for the specified key. Alias for hasKey.
     */
    has(key: K): boolean {
        return this.data.has(key)
    }

    /**
     * Returns true if this map contains a mapping for the specified key.
     */
    hasKey(key: K): boolean {
        return this.data.has(key)
    }

    /**
     * Returns true if the specified value is associated with the specified key in this map.
     */
    hasValue(key: K, value: V): boolean {
        return this.data.get(key)?.has(value) ?? false
    }

    /**
     * Returns an iterator over the keys in this map.
     */
    keys(): IterableIterator<K> {
        return this.data.keys()
    }

    /**
     * Returns an iterator over the sets of values in this map.
     */
    values(): IterableIterator<Set<V>> {
        return this.data.values()
    }

    /**
     * Returns an iterable of key, value pairs for every entry in the map.
     */
    entries(): IterableIterator<[K, Set<V>]> {
        return this.data.entries()
    }

    /**
     * Removes all of the mappings from this map.
     */
    clear(): void {
        this.data.clear()
    }

    /**
     * Executes a provided function once per each key/value pair in the map.
     */
    forEach(callback: (value: Set<V>, key: K, map: Map<K, Set<V>>) => void): void {
        this.data.forEach(callback)
    }
}
