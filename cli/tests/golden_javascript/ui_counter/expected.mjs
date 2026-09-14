const $k0=[1200n,'lay-col'];
const $k1=[$k0];
const $k2=[$k1];
const $k3=[5,$k2];
const $k4=[$k3];
const $k5=[0,false];
const $k6=[0,'doubled'];
const $k7=[1200n,'lay-row'];
const $k8=[4200n,'px-r0_5'];
const $k9=[7400n,'r-6'];
const $k10=[6200n,'bg-f0f0f5'];
const $k11=[6400n,'fg-18181b'];
const $k12=[6205n,'hover_bg-18181b'];
const $k13=[6405n,'hover_fg-f0f0f5'];
const $k14=[$k7,$k8,$k9,$k10,$k11,$k12,$k13];
const $k15=[$k14];
const $k16=[5,$k15];
const $k17=[$k16];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n:where(button){appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n';
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const self_3=$host_HostStdout_println(ctx_0[1],'mounted');
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const label_7='clicks';
  const count_8=[$host_HostUi_signal(ctx_0[2],0n)];
  const config_17=[[0,label_7],[],void 0,c_9=>$host_HostUi_write(c_9[2],count_8[0],(n_10=>n_10+1n)($host_HostUi_read(c_9[2],count_8[0]))),void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
  const events_18=ui_node$listen$u3rqgv(config_17[8],config_17[9],config_17[10],config_17[11],config_17[12]);
  let $t3;
  const $t4=config_17[4];
  if($t4!==void 0&&$t4===1){
    $t3=[[14,config_17[0],$share(config_17[1]),events_18]];
  }else{
    const $t8=$share(config_17[1]);
    const children_19=$share(config_17[2]);
    let $t5;
    if(children_19!==void 0){
      $t5=$share(children_19);
    }else if(children_19===void 0){
      $t5=[];
    }else{
      $abort('no arm matched');
    }
    let $t9;
    const $t10=config_17[3];
    if($t10!==void 0){
      $t9=(c_23,_event_24)=>$t10(c_23);
    }else if($t10===void 0){
      $t9=(_c_25,_event_26)=>0;
    }else{
      $abort('no arm matched');
    }
    let $t11;
    const $t12=config_17[5];
    if($t12!==void 0){
      $t11=$t12;
    }else if($t12===void 0){
      $t11=$k5;
    }else{
      $abort('no arm matched');
    }
    let $t13;
    const $t14=config_17[6];
    if($t14!==void 0){
      $t13=$t14;
    }else if($t14===void 0){
      $t13=$k5;
    }else{
      $abort('no arm matched');
    }
    let $t15;
    const $t16=config_17[7];
    if($t16!==void 0){
      $t15=$t16;
    }else if($t16===void 0){
      $t15=$k5;
    }else{
      $abort('no arm matched');
    }
    $t3=[[5,config_17[0],$t8,$t5,$t9,$t11,$t13,$t15,events_18]];
  }
  return $ui_node_mount(ctx_0,ui_node$stack$u3rqgv([$k4,[$t3,__cmd_x_main_buri$badge$u3rqgv([0,label_7],[1,count_8]),__cmd_x_main_buri$badge$u3rqgv($k6,[2,c_11=>ui_signal$Signal_get$xiaice(count_8,c_11)*2n])],void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function __cmd_x_main_buri$badge$u3rqgv(title_0,count_1){
  return ui_node$stack$u3rqgv([$k17,[ui_node$text$u3rqgv([title_0,void 0,void 0]),ui_node$text$u3rqgv([[2,c_2=>{
    let $t1;
    if(count_1[0]===0){
      $t1=count_1[1];
    }else if(count_1[0]===1){
      $t1=$ui_effect_Scope_read(c_2,count_1[1][0]);
    }else if(count_1[0]===2){
      $t1=count_1[1](c_2);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }],void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0]);
}
function ui_signal$Signal_get$xiaice(self_0,ctx_1){
  return $ui_effect_Scope_read(ctx_1,self_0[0]);
}
function ui_node$stack$u3rqgv(config_0){
  const events_1=[config_0[3],config_0[4],config_0[5],config_0[6],config_0[7]];
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
function ui_node$listen$u3rqgv(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4){
  return [onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4];
}
function ui_node$text$u3rqgv(config_0){
  const $t1=config_0[1];
  if($t1!==void 0){
    const styles_2=$share(config_0[2]);
    let $t2;
    if(styles_2!==void 0){
      $t2=$share(styles_2);
    }else if(styles_2===void 0){
      $t2=[];
    }else{
      $abort('no arm matched');
    }
    return [[2,$t1,$t2,config_0[0]]];
  }else if($t1===void 0){
    return [[1,config_0[0]]];
  }else{
    $abort('no arm matched');
  }
}
