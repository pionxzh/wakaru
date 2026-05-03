import { isNull, isUndefined } from '@wakaru/ast-utils/matchers'
import { removeDeclarationIfUnused } from '@wakaru/ast-utils/scope'
import type { ExpressionKind } from 'ast-types/lib/gen/kinds'
import type { ASTPath, BinaryExpression, ConditionalExpression, Identifier, JSCodeshift, LogicalExpression } from 'jscodeshift'

/**
 * Information about a property in a chain
 */
interface PropertyInfo {
  propertyName: string
  computed: boolean
}

/**
 * Information about a variable assignment in a chain
 */
interface VarAssignment {
  varName: string
  parent: string | null
  source: ExpressionKind
  properties: PropertyInfo[]
}

/**
 * Extract optional chain structure from a complex nested nullish coalescing pattern.
 * 
 * Pattern:
 * null !== (t = null == r || null === (n = r.app_info) || void 0 === n || null === (o = n.base_info) || void 0 === o
 *            ? void 0
 *            : o.app_name) && void 0 !== t ? t : "game"
 * 
 * Should be transformed to:
 * r?.app_info?.base_info?.app_name ?? "game"
 */
export function analyzeOptionalChain(
  j: JSCodeshift,
  path: ASTPath<ConditionalExpression>,
  cleanUpVariables = true
): ExpressionKind | null {
  // Only handle conditional expressions for nullish coalescing patterns
  if (!j.ConditionalExpression.check(path.node)) return null
  
  const { test, consequent, alternate } = path.node
  
  // Look for the pattern: null !== (t = ...) && void 0 !== t ? t : "fallback"
  if (!j.LogicalExpression.check(test) || test.operator !== '&&') return null
  
  const mainAssignmentCheck = test.left
  
  // Check main assignment: null !== (t = ...)
  if (!j.BinaryExpression.check(mainAssignmentCheck) ||
      !['!==', '!='].includes(mainAssignmentCheck.operator) ||
      !isNull(j, mainAssignmentCheck.left) ||
      !j.AssignmentExpression.check(mainAssignmentCheck.right)) {
    return null
  }
  
  const tempVar = mainAssignmentCheck.right.left
  if (!j.Identifier.check(tempVar)) return null
  
  // Check if the identifier is used in the conditional: t : "fallback"
  if (!j.Identifier.check(consequent) || tempVar.name !== consequent.name) return null
  
  // Extract the nested conditional: null == r || null === (n = r.app_info) || ...
  const nestedConditional = mainAssignmentCheck.right.right
  if (!j.ConditionalExpression.check(nestedConditional)) return null
  
  // The consequent of the nested conditional should be void 0
  if (!isUndefined(j, nestedConditional.consequent)) return null
  
  // The alternate of the nested conditional should be a property access
  if (!j.MemberExpression.check(nestedConditional.alternate)) return null
  
  // Extract the nested OR conditions
  const orConditions = extractLogicalOrConditions(j, nestedConditional.test)
  
  // Extract variable assignments and their relationships
  const { rootVar, varMap } = analyzeConditions(j, orConditions)
  if (!rootVar) return null
  
  // Create the optional chain from root to the final property access
  const finalExpr = buildOptionalChain(j, rootVar, varMap, nestedConditional.alternate)
  if (!finalExpr) return null
  
  // Create the nullish coalescing expression
  const result = j.logicalExpression('??', finalExpr, alternate)
  
  // Clean up temporary variables
  if (cleanUpVariables) {
    cleanupTemporaryVariables(j, path, varMap)
  }
  
  return result
}

/**
 * Extract all conditions from a logical OR expression
 */
function extractLogicalOrConditions(j: JSCodeshift, expr: ExpressionKind): ExpressionKind[] {
  if (j.LogicalExpression.check(expr) && expr.operator === '||') {
    return [...extractLogicalOrConditions(j, expr.left), ...extractLogicalOrConditions(j, expr.right)]
  }
  return [expr]
}

/**
 * Analyze conditions to extract variable assignments and their relationships
 */
function analyzeConditions(
  j: JSCodeshift, 
  conditions: ExpressionKind[]
): { 
  rootVar: string | null, 
  varMap: Map<string, VarAssignment>
} {
  const varMap = new Map<string, VarAssignment>()
  let rootVar: string | null = null
  
  for (const condition of conditions) {
    if (j.BinaryExpression.check(condition)) {
      // Handle conditions like: null == r, null === (n = r.app_info), void 0 === n
      const { left, right, operator } = condition
      
      // Skip if not a null/undefined check
      if (!(['==', '===', '!=', '!=='].includes(operator) && 
            (isNull(j, left) || isNull(j, right) || 
             isUndefined(j, left) || isUndefined(j, right)))) {
        continue
      }
      
      // Get the non-null part of the expression
      const expr = isNull(j, left) || isUndefined(j, left) ? right : left
      
      // Direct variable check: null == r
      if (j.Identifier.check(expr) && !rootVar) {
        rootVar = expr.name
        varMap.set(rootVar, {
          varName: rootVar,
          parent: null,
          source: expr,
          properties: []
        })
      }
      
      // Property assignment: null === (n = r.app_info)
      if (j.AssignmentExpression.check(expr) && j.Identifier.check(expr.left)) {
        const assignedVar = expr.left.name
        const sourceExpr = expr.right
        
        if (j.MemberExpression.check(sourceExpr)) {
          // Extract the object and property from the member expression
          const objExpr = sourceExpr.object
          let parentVar: string | null = null
          
          if (j.Identifier.check(objExpr)) {
            parentVar = objExpr.name
            
            // If this is the first assignment from the root variable, establish the root
            if (!rootVar && !varMap.has(parentVar)) {
              rootVar = parentVar
              varMap.set(rootVar, {
                varName: rootVar,
                parent: null,
                source: objExpr,
                properties: []
              })
            }
          }
          
          // Extract property information
          const property = sourceExpr.property
          const propertyName = j.Identifier.check(property) ? property.name : j(property).toSource()
          
          // Record this variable and its property access
          varMap.set(assignedVar, {
            varName: assignedVar,
            parent: parentVar,
            source: sourceExpr,
            properties: [{
              propertyName,
              computed: sourceExpr.computed
            }]
          })
        }
      }
    }
  }
  
  return { rootVar, varMap }
}

/**
 * Build an optional chain from the root variable to the final property access
 */
function buildOptionalChain(
  j: JSCodeshift, 
  rootVar: string, 
  varMap: Map<string, VarAssignment>,
  finalAccess: ExpressionKind
): ExpressionKind | null {
  // Extract the property chain from the variable map
  const propertyChain: PropertyInfo[] = []
  
  // Add the final property access
  if (j.MemberExpression.check(finalAccess)) {
    const obj = finalAccess.object
    const prop = finalAccess.property
    
    if (j.Identifier.check(obj) && varMap.has(obj.name)) {
      // Add the final property to the chain
      if (j.Identifier.check(prop)) {
        propertyChain.push({
          propertyName: prop.name,
          computed: finalAccess.computed
        })
      } else {
        propertyChain.push({
          propertyName: j(prop).toSource(),
          computed: finalAccess.computed
        })
      }
      
      // Trace back through the variable assignments to build the full chain
      let currentVar = obj.name
      while (currentVar && varMap.has(currentVar)) {
        const varInfo = varMap.get(currentVar)!
        
        // Add all properties from this variable
        varInfo.properties.forEach(prop => {
          propertyChain.unshift(prop)
        })
        
        // Move to the parent variable
        currentVar = varInfo.parent || ''
      }
    }
  }
  
  // Build the optional chain expression
  if (propertyChain.length > 0) {
    // Start with the root object
    let expr: ExpressionKind = j.identifier(rootVar)
    
    // Add each property with optional chaining
    for (const { propertyName, computed } of propertyChain) {
      const property = computed || !isValidIdentifier(propertyName)
        ? j.literal(propertyName)
        : j.identifier(propertyName)
        
      expr = j.optionalMemberExpression(
        expr,
        property,
        computed
      )
    }
    
    return expr
  }
  
  return null
}

/**
 * Clean up temporary variables created during minification
 */
function cleanupTemporaryVariables(
  j: JSCodeshift,
  path: ASTPath,
  varMap: Map<string, VarAssignment>
): void {
  // Clean up all the temporary variables
  for (const [varName] of varMap) {
    removeDeclarationIfUnused(j, path, varName)
  }
}

/**
 * Check if a string is a valid JavaScript identifier
 */
function isValidIdentifier(str: string): boolean {
  // Simple check - in a real implementation, we'd use a more robust regex
  return /^[a-zA-Z_$][a-zA-Z0-9_$]*$/.test(str)
}
