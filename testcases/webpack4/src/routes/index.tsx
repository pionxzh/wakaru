import * as React from "react";
import { Route, Switch } from "react-router-dom";

import pages from "../pages";

const routes: JSX.Element = (
    <Switch>
        <Route exact={true} path="/" component={pages.FrontPage} />
    </Switch>
);

export default routes;
