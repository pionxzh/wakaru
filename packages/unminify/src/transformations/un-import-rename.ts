import { assertScopeExists } from '@wakaru/ast-utils/assert'
import { generateName } from '@wakaru/ast-utils/identifier'
import { renameIdentifier } from '@wakaru/ast-utils/reference'
import { createJSCodeshiftTransformationRule } from '@wakaru/shared/rule'
import type { Identifier } from 'jscodeshift'

/**
 * Rename import specifier back to the original name
 *
 * @example
 * import { foo as ab } from 'bar';
 * ab();
 * ->
 * import { foo } from 'bar'
 * foo();
 */
export default createJSCodeshiftTransformationRule({
    name: 'un-import-rename',
    transform: (context) => {
        const { root, j } = context

        root
            .find(j.ImportSpecifier, {
                imported: {
                    type: 'Identifier',
                },
                local: {
                    type: 'Identifier',
                },
            })
            .paths()
            .forEach((path) => {
                const specifier = path.node
                const imported = specifier.imported as Identifier
                const local = specifier.local as Identifier

                if (imported.name === local.name) return

                const scope = path.scope
                assertScopeExists(scope)

                const targetName = generateName(imported.name, scope)
                renameIdentifier(j, scope, local.name, targetName)
            })

        /**
         * fix all import { foo as foo } to import { foo }
         *
         * Doing this in a separate loop to avoid modifying the AST structure while iterating over it
         * It will cause some weird behavior
         */
        root
            .find(j.ImportSpecifier, {
                imported: {
                    type: 'Identifier',
                },
                local: {
                    type: 'Identifier',
                },
            })
            .forEach((path) => {
                const specifier = path.node
                const imported = specifier.imported as Identifier
                const local = specifier.local as Identifier

                if (imported.name !== local.name) return

                const newIdent = j.identifier(imported.name)
                path.replace(j.importSpecifier(newIdent, newIdent))
            })
    },
})
