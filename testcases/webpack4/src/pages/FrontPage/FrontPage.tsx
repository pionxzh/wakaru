import * as React from "react";
import { IFrontPage } from "./";

class FrontPage extends React.Component<IFrontPage, {}> {
    componentDidMount(): void {
        this.props.fetchData();
    }

    render(): JSX.Element {
        return (
            <div>
                <h1>Front page</h1>
            </div>
        );
    }
}

export default FrontPage;
