import { START, SUCCESS } from '../commonActionTypes'
import { FETCH_DATA } from './MainTypes'
import type { IActionCreator } from '../../interfaces/IReducers'

export const startFetchingData: IActionCreator = () => ({
    type: FETCH_DATA + START,
})

export const fetchDataSuccess: IActionCreator<string[]> = payload => ({
    type: FETCH_DATA + SUCCESS,
    payload,
})

export const fetchData: IActionCreator = () => ({
    type: FETCH_DATA,
})
