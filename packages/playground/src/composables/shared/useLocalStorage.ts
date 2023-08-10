import { useEventListener } from '@vueuse/core'
import { computed, nextTick, watch } from 'vue'
import useState from './useState'
import { getDefaultSerializer } from './useStorage'
import type { Serializer } from './useStorage'
import type { Ref } from 'vue'

function hookBefore(target: any, methodName: string, hookFn: Function) {
    const original = target[methodName]
    target[methodName] = function (...args: any[]) {
        hookFn(...args)
        return original.apply(this, args)
    }
}

function tryPatchLocalStorage() {
    // @ts-expect-error mark it as patched
    if (localStorage.setItem.__patched__) return
    // @ts-expect-error mark it as patched
    localStorage.setItem.__patched__ = true

    hookBefore(localStorage, 'setItem', (key: string, newValue: string) => {
        const oldValue = localStorage.getItem(key)
        const event = new StorageEvent('storage', {
            key,
            newValue,
            oldValue,
            storageArea: localStorage,
        })
        nextTick(() => window.dispatchEvent(event))
    })

    hookBefore(localStorage, 'removeItem', (key: string) => {
        const oldValue = localStorage.getItem(key)
        const event = new StorageEvent('storage', {
            key,
            newValue: undefined,
            oldValue,
            storageArea: localStorage,
        })
        nextTick(() => window.dispatchEvent(event))
    })
}

function tryParseFn<T>(decode: (input: string) => T) {
    return (str: string | null, defaultValue: T): T => {
        try {
            if (str === '' || str === null || str === undefined) {
                return defaultValue
            }

            return decode(str)
        }
        catch (err) {
            console.error('useLocalStorage parse failed with value:', str, err)
            return defaultValue
        }
    }
}

export function useLocalStorage<T>(key: string, defaultValue: T, options?: Serializer<T>): [Ref<T>, (newVal: T) => void] {
    tryPatchLocalStorage()

    const defaultSerializer = getDefaultSerializer(defaultValue)
    const decode = options?.decode ?? defaultSerializer.decode
    const encode = options?.encode ?? defaultSerializer.encode

    const initValue = localStorage.getItem(key) || ''
    const [rawValue, setRawValue] = useState<string>(initValue)
    watch(rawValue, (value) => {
        return localStorage.setItem(key, value)
    })

    const tryParse = tryParseFn(decode)
    const target = computed<T>(() => tryParse(rawValue.value, defaultValue))

    const setValue = (newValue: T) => {
        const encoded = encode(newValue)
        if (rawValue.value !== encoded) {
            localStorage.setItem(key, encoded)
            setRawValue(encoded)
        }
    }
    const value = computed({
        get: () => target.value,
        set: setValue,
    })

    const onStorage = (event: StorageEvent): void => {
        if (event.storageArea === localStorage && event.key === key) {
            const newValue = event.newValue || ''
            if (newValue !== rawValue.value) {
                setRawValue(newValue)
            }
        }
    }

    useEventListener(window, 'storage', onStorage)

    return [value, setValue]
}
