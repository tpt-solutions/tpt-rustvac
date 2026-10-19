fn main() {
    let mut wv = webview::new(webview::WebViewBuilder::new().title("tpt-rustvac").size(1100, 640).debug(false).content(webview::Content::Html("<h1 style='font-family:sans-serif'>hello</h1>")).user_data(()).invoke_handler(|_wv, _arg| Ok(())).build().expect("webview"));
    wv.run().expect("run");
}
