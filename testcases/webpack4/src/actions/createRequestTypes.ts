const REQUEST = 'REQUEST'
const SUCCESS = 'SUCCESS'
const FAILURE = 'FAILURE'

export interface IRequestTypes {
    [key: string]: string
}

function createRequestTypes(base: string): IRequestTypes {
    return [REQUEST, SUCCESS, FAILURE].reduce((acc: IRequestTypes, type) => {
        acc[type] = `${base}_${type}`
        return acc
    }, {})
}

export default createRequestTypes
