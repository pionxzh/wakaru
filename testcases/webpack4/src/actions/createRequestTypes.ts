const REQUEST: string = "REQUEST";
const SUCCESS: string = "SUCCESS";
const FAILURE: string = "FAILURE";

export interface IRequestTypes {
    [key: string]: string;
}

function createRequestTypes(base: string): IRequestTypes {
    return [REQUEST, SUCCESS, FAILURE].reduce((acc: IRequestTypes, type) => {
        acc[type] = `${base}_${type}`;
        return acc;
    }, {});
}

export default createRequestTypes;
