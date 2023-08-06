import { all, put, take } from 'redux-saga/effects'

import { fetchDataSuccess, startFetchingData } from '../actions/MainActions'
import { FETCH_DATA } from '../actions/MainActions/MainTypes'
import type { SagaIterator } from 'redux-saga'

function* watchFetchData(): SagaIterator {
    while (true) {
        yield take(FETCH_DATA)
        yield put(startFetchingData())
        yield put(fetchDataSuccess([]))
    }
}

function* rootSaga(): any {
    yield all([
        watchFetchData(),
    ])
}

export default rootSaga
