import { ERROR, START, SUCCESS } from '../../actions/commonActionTypes'
import { FETCH_DATA } from '../../actions/MainActions/MainTypes'
import type { IActionHandler, IActionHandlers, IActionObject } from '../../interfaces/IReducers'

export interface IMainReducer {
    isLoading: boolean
    data: string[]
    error: string
}

const initialState: IMainReducer = {
    isLoading: false,
    data: [],
    error: '',
}

const startFetching: IActionHandler<IMainReducer> = state => Object.assign({}, state, { isLoading: true })

const fetchSuccess: IActionHandler<IMainReducer> = (state, payload) => Object.assign({}, state, { isLoading: false, data: payload })

const fetchError: IActionHandler<IMainReducer> = (state, payload) => Object.assign({}, state, { isLoading: false, error: payload })

const reducerHandler: IActionHandlers<IMainReducer> = {
    [FETCH_DATA + START]: startFetching,
    [FETCH_DATA + SUCCESS]: fetchSuccess,
    [FETCH_DATA + ERROR]: fetchError,
}

export default (state = initialState, action: IActionObject): IMainReducer => {
    const reducer: IActionHandler<IMainReducer> = reducerHandler[action.type]

    return reducer ? reducer(state, action.payload) : state
}
