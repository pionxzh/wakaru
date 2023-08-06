import { connect } from 'react-redux'
import { compose } from 'redux'

import { fetchData } from '../../actions/MainActions/index'
import FrontPage from './FrontPage'
import type { IActionCreator } from '../../interfaces/IReducers'
import type { IGlobalStore } from '../../reducers/index'

import type { MapDispatchToProps, MapStateToProps } from 'react-redux'

interface IStateProps {
    data: string[]
}
interface IDispatchProps {
    fetchData: IActionCreator
}

export type IFrontPage = IStateProps & IDispatchProps

const mapStateToProps: MapStateToProps<IStateProps, {}, IGlobalStore> = ({ main }) => ({
    data: main.data,
})

const mapDispatchToProps: MapDispatchToProps<IDispatchProps, {}> = {
    fetchData,
}

export default compose(
    connect(mapStateToProps, mapDispatchToProps),
)(FrontPage)
