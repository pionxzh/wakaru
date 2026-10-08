import { Box, Shop, Mode, counter, bump } from "./a";
const { ccclass, property } = cc._decorator;

@ccclass
export default class Main extends cc.Component {
    @property
    label: string = "main";

    start() {
        bump();
        console.log(Box.read(), Shop.make(), Mode.B, counter);
    }
}
