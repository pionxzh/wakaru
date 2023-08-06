import { createBrowserHistory } from 'history'
import { routerMiddleware } from 'react-router-redux'
import { applyMiddleware, createStore } from 'redux'
import createSagaMiddleware from 'redux-saga'

import reducer from '../reducers'
import saga from '../sagas'
import type { History } from 'history'
import type { Middleware, Store } from 'redux'
import type { SagaMiddleware } from 'redux-saga'

export const history: History = createBrowserHistory()

const sagaMiddleware: SagaMiddleware<Middleware> = createSagaMiddleware()
const middleware: Middleware[] = [routerMiddleware(history), sagaMiddleware]

const store: Store = createStore(reducer, applyMiddleware(...middleware))

sagaMiddleware.run(saga)

export default store
