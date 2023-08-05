import * as React from "react";
import { compose } from "redux";
import { withRouter } from "react-router-dom";

import routes from "./routes";

class App extends React.Component<any, {}> {
    render(): JSX.Element {
        console.log(this.props);

        return (
            <div>
                {routes}
            </div>
        );
    }
}

export default compose(
    withRouter,
)(App);
