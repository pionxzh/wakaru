import { version } from './b.js'

export const getC = async () => {
    console.log('c.a', version)
    const result = await fetch('https://jsonplaceholder.typicode.com/todos/1')
    const json = await result.json()
    return json
}
