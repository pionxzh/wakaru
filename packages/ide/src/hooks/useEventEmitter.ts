import mitt from 'mitt'
import { useEffect } from 'react'

interface EventMap {
    'file:open': {
        path: string
    }
    'file:rename': {
        path: string
        newName: string
    }
}

// @ts-expect-error EventMap will conflict with the one from mitt
export const eventEmitter = mitt<EventMap>()

export function useEventSubscriber<K extends keyof EventMap>(key: K, handler: (data: EventMap[K]) => void) {
    useEffect(() => {
        eventEmitter.on(key, handler)
        return () => eventEmitter.off(key, handler)
    }, [key, handler])
}

export function useEventPublisher<K extends keyof EventMap>(key: K) {
    return (data: EventMap[K]) => {
        eventEmitter.emit(key, data)
    }
}
