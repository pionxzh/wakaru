import logo from '/icon.png?url'

export function Header() {
    return (
        <div
            className="h-full"
            style={{ backgroundColor: 'rgb(40, 44, 52)' }}
        >
            <div
                className="h-full w-8 bg-no-repeat bg-center bg-[length:16px]"
                style={{
                    backgroundImage: `url(${logo})`,
                }}
            />
        </div>
    )
}
