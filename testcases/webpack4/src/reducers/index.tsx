import { routerReducer } from 'react-router-redux'
import { combineReducers } from 'redux'
import main from './MainReducer'
import type { IMainReducer } from './MainReducer'
import type { IActionObject } from '../interfaces/IReducers'
import type { RouterState } from 'react-router-redux'

// Reducers
import type { Reducer } from 'redux'

// Reducers Interfaces
export interface IGlobalStore {
    router: RouterState
    main: IMainReducer
}

const reducer: Reducer<IGlobalStore, IActionObject> = combineReducers({
    router: routerReducer,
    main,
})

export default reducer
