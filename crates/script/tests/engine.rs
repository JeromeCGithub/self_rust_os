use rustos_script::{Error,Host,Runtime,Target,Value};
#[derive(Default)]
struct FakeDom {title:String}
impl Host for FakeDom {
    fn read(&mut self,target:&Target)->Result<Value,Error> {match target {Target::Title=>Ok(Value::Text(self.title.clone())),_=>Err(Error::MissingElement)}}
    fn write(&mut self,target:&Target,value:Value)->Result<(),Error> {match target {Target::Title=>{self.title=value.text();Ok(())},_=>Err(Error::MissingElement)}}
}
#[test]
fn precedence_unary_decimal_comments_getters_and_string_escaping() {
    let mut dom=FakeDom::default();let mut runtime=Runtime::new();
    runtime.execute("/* computation */ let n=-(2+3)*4/2+0.5; document.title='value: '+n; // end",&mut dom).unwrap();
    assert_eq!(dom.title,"value: -9.5");
    runtime.execute(r#"document.title=document.title+'\n'+true+null;"#,&mut dom).unwrap();
    assert_eq!(dom.title,"value: -9.5\ntruenull");
}
#[test]
fn source_tokens_nesting_variables_and_strings_have_independent_budgets() {
    let mut dom=FakeDom::default();
    for source in [" ".repeat(32769),";".repeat(4097),format!("document.title={}1{};","(".repeat(33),")".repeat(33)),format!("document.title='{}';","x".repeat(8193)),(0..129).map(|n|format!("let x{n}=1;")).collect::<String>()] {
        assert_eq!(Runtime::new().execute(&source,&mut dom),Err(Error::Budget));
    }
    let mut runtime=Runtime::new();
    runtime.execute(&" ".repeat(20000),&mut dom).unwrap();
    assert_eq!(runtime.execute(&" ".repeat(20000),&mut dom),Err(Error::Budget));
}
#[test]
fn failures_stop_one_script_without_destroying_its_realm() {
    let mut dom=FakeDom::default();let mut runtime=Runtime::new();
    assert_eq!(runtime.execute("let x=4; missing=2; document.title='unreachable';",&mut dom),Err(Error::UnknownName));
    assert_eq!(dom.title,"");
    runtime.execute("document.title=x+1;",&mut dom).unwrap();assert_eq!(dom.title,"5");
    assert_eq!(runtime.execute("let invalid=;",&mut dom),Err(Error::Syntax));
    assert_eq!(runtime.execute("document.title=1/0;",&mut dom),Err(Error::Type));
}
