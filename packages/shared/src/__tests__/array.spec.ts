import { expect, it } from 'vitest'
import { arraify } from '../array'

it('should always return an array', () => {
    expect(arraify(1)).toEqual([1])
    expect(arraify([1])).toEqual([1])
})
