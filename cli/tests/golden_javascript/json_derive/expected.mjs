const $k0=[1,2];
const $k1=[1,3,4];
const $k2=[0];
const $k3=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
const $D4=[];
const $D5=[];
$D0.push(3,'Result',[['Ok',false,['0'],[$D1]],['Err',false,['0'],[$D3]]],false);
$D1.push(2,'Point',true,['x','y'],[$D2,$D2]);
$D2.push(0,'i');
$D3.push(3,'DecodeError',[['Missing',true,['path'],[$D4]],['WrongType',true,['path','wanted','found'],[$D4,$D4,$D4]],['UnknownVariant',true,['path','tag'],[$D4,$D4]]],false);
$D4.push(0,'s');
$D5.push(3,'Shape',[['Empty',false,[],[]],['Rect',true,['width','height'],[$D2,$D2]]],false);
function $eqD0(a,b){
  if(a===b){
    return true;
  }
  if(a[0]!==b[0]){
    return false;
  }
  switch(a[0]){
    case 0:
      return $eqD1(a[1],b[1]);
    case 1:
      return $eqD3(a[1],b[1]);
    default:
      return false;
  }
}
function $eqD1(a,b){
  if(a===b){
    return true;
  }
  return a[0]===b[0]&&a[1]===b[1];
}
function $eqD3(a,b){
  if(a===b){
    return true;
  }
  if(a[0]!==b[0]){
    return false;
  }
  switch(a[0]){
    case 0:
      return a[1]===b[1];
    case 1:
      return a[1]===b[1]&&a[2]===b[2]&&a[3]===b[3];
    case 2:
      return a[1]===b[1]&&a[2]===b[2];
    default:
      return false;
  }
}
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],core_json$stringify$u3rqgv(ctx_0,$json_of($k0,$D1)));
  $host_HostStdout_println(ctx_0[1],core_json$stringify$u3rqgv(ctx_0,$json_of($k1,$D5)));
  $host_HostStdout_println(ctx_0[1],core_json$stringify$u3rqgv(ctx_0,$json_of($k2,$D5)));
  const back_3=$json_decode($json_of($k0,$D1),$D1);
  $host_HostStdout_println(ctx_0[1],$str($eqD0(back_3,[0,$k0])));
  return $k3;
}
function core_json$stringify$u3rqgv(ctx_0,v_1){
  switch(v_1[0]){
    case 0:
      {
        return 'null';
      }
    case 1:
      {
        return v_1[1]?'true':'false';
      }
    case 2:
      {
        const s_17=$str_fromFloat(ctx_0,v_1[1]);
        return $str_endsWith(s_17,'.0')?$str_slice(s_17,0,$str_len(s_17)-2):s_17;
      }
    case 3:
      {
        return core_json$quote$u3rqgv(ctx_0,v_1[1]);
      }
    case 4:
      {
        const items_5=v_1[1];
        $share(items_5);
        const parts_8=$list_mapCtx(items_5,ctx_0,(c_6,item_7)=>core_json$stringify$u3rqgv(c_6,item_7));
        return $str_format(ctx_0,'['+$list_join(parts_8,ctx_0,',')+']');
      }
    case 5:
      {
        const entries_9=v_1[1];
        $share(entries_9);
        const parts_14=$list_mapCtx(entries_9,ctx_0,(c_10,e_11)=>$str_format(c_10,core_json$quote$u3rqgv(c_10,e_11[0])+':'+core_json$stringify$u3rqgv(c_10,e_11[1])));
        return $str_format(ctx_0,'{'+$list_join(parts_14,ctx_0,',')+'}');
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function core_json$quote$u3rqgv(ctx_0,s_1){
  const inner_4=$list_join($list_mapCtx($str_chars(s_1,ctx_0),ctx_0,(c_2,ch_3)=>{
    if(ch_3==='"'){
      return '\\"';
    }else if(ch_3==='\\'){
      return '\\\\';
    }else if(ch_3==='\n'){
      return '\\n';
    }else if(ch_3==='\r'){
      return '\\r';
    }else if(ch_3==='\t'){
      return '\\t';
    }else if($char_toU32(ch_3)<32){
      const n_7=$char_toU32(ch_3);
      const self_8=$str_charAt('0123456789abcdef',Math.trunc(n_7/16));
      let $t1;
      if(self_8!==void 0){
        $t1=self_8;
      }else if(self_8===void 0){
        $t1='0';
      }else{
        $abort('no arm matched');
      }
      const self_11=$str_charAt('0123456789abcdef',n_7%16);
      let $t3;
      if(self_11!==void 0){
        $t3=self_11;
      }else if(self_11===void 0){
        $t3='0';
      }else{
        $abort('no arm matched');
      }
      return $str_format(c_2,'\\u00'+$t1+$t3);
    }else{
      return $str_fromChars(c_2,[ch_3]);
    }
  }),ctx_0,'');
  return $str_format(ctx_0,'"'+inner_4+'"');
}
