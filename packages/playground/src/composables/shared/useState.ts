import { ref } from 'vue'
import type { Ref } from 'vue'

/**
 * Returns a stateful value, and a function to update it.
 * @example
 * const [count, setCount] = useState(0)
 * setCount(99)
 * @example
 * const [count, setCount] = useState(0)
 * // setCount(count.value + 1)
 * setCount(c => c + 1) // can simplify to this
 */
export default function useState<S>(initialState: S): [Ref<S>, (newVal: S | ((previousValue: S) => S)) => void] {
    const state = ref(initialState) as Ref<S>
    const setState = (newValue: S | ((previousValue: S) => S)) => {
        if (newValue instanceof Function) {
            state.value = newValue(state.value)
        }
        else {
            state.value = newValue
        }
    }

    return [state, setState]
}
