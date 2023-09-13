import { expect, it } from 'vitest'
import { generateName as fn } from '../identifier'

it('should generate a identifier name', () => {
    expect(fn('foo')).toBe('foo')
    expect(fn('foo-bar')).toBe('fooBar')
    expect(fn('foo.bar')).toBe('fooBar')
    expect(fn('@foo/bar')).toBe('fooBar')
    expect(fn('@foo/bar-baz')).toBe('fooBarBaz')
    expect(fn('@foo/bar.baz')).toBe('fooBarBaz')
    expect(fn('./foo')).toBe('foo')
    expect(fn('./nested/foo')).toBe('nestedFoo')
    expect(fn('./nested/foo-bar')).toBe('nestedFooBar')
    expect(fn('./nested/foo.bar')).toBe('nestedFooBar')
    expect(fn('../nested/foo-bar-baz')).toBe('nestedFooBarBaz')
    expect(fn('../deep/nested/foo.bar.baz')).toBe('nestedFooBarBaz')
})

it('should generate a valid identifier name', () => {
    expect(fn('import')).toBe('_import')
    expect(fn('const')).toBe('_const')
})
