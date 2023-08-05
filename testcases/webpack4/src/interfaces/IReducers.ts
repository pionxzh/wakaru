import { Dispatch, AnyAction, Action } from "redux";

export type IActionHandler<State> = (state: State, payload?: any) => State;

export interface IActionHandlers<State> {
    [key: string]: IActionHandler<State>;
}

export interface IActionObject<P = null> {
    type: string;
    payload?: P;
}

export type IActionCreator<P = null> = (payload?: P) => IActionObject<P>;

export type ICommonAction<A1 = null, A2 = null, A3 = null, A4 = null> =
    (data1?: A1, data2?: A2, data3?: A3, data4?: A4) => (dispatch: Dispatch<any>, getState?: any) => void;
