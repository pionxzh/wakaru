import { Reducer, combineReducers } from "redux";
import { routerReducer, RouterState } from "react-router-redux";
import { IActionObject } from "../interfaces/IReducers";

// Reducers
import main, { IMainReducer } from "./MainReducer";

// Reducers Interfaces
export interface IGlobalStore {
    router: RouterState;
    main: IMainReducer;
}

const reducer: Reducer<IGlobalStore, IActionObject> = combineReducers({
    router: routerReducer,
    main,
});

export default reducer;
