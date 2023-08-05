import { SagaIterator } from "redux-saga";
import { put, all, call, take } from "redux-saga/effects";

import { startFetchingData, fetchDataSuccess } from "../actions/MainActions";
import { FETCH_DATA } from "../actions/MainActions/MainTypes";

function* watchFetchData(): SagaIterator {
    while (true) {
        yield take(FETCH_DATA);
        yield put(startFetchingData());
        yield put(fetchDataSuccess([]));
    }
}

function* rootSaga(): any {
    yield all([
        watchFetchData(),
    ]);
}

export default rootSaga;
