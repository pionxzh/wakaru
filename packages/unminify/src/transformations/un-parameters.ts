import { mergeTransformationRule } from '@wakaru/shared/rule'
import unDefaultParameter from './un-default-parameter'
import unParameterRest from './un-parameter-rest'

/**
 * Restore parameter syntax.
 *
 * @see https://babeljs.io/docs/babel-plugin-transform-parameters
 */
export default mergeTransformationRule('un-parameters', [unDefaultParameter, unParameterRest])
